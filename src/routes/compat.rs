use std::{convert::Infallible, time::Instant};

use axum::{
    body::Bytes,
    extract::{Path, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    app::AppState,
    compat::{openai_id, unix_timestamp},
    error::AppError,
    models::{
        chat::TokenUsage,
        responses::{
            ResponseCompleted, ResponseObject, ResponseOutput, ResponseOutputContent,
            ResponseRequest, ResponseStreamEvent, ResponseTextDelta, ResponseUsage,
        },
    },
    providers::provider::ProviderStreamEvent,
    routing::{
        fallback::{self, FallbackError},
        strategy::resolve_model_alias,
    },
};

const MAX_STREAM_OUTPUT_CHARS: usize = 256 * 1024;

pub async fn responses(
    State(state): State<AppState>,
    Json(request): Json<ResponseRequest>,
) -> Result<Response, AppError> {
    let request_id = Uuid::new_v4();
    let started = Instant::now();
    let stream = request.stream_enabled();
    let mut chat_request = request.into_chat_request();
    if let Some(model) = chat_request.model.as_deref() {
        chat_request.model = Some(resolve_model_alias(&state.model_aliases, model));
    }
    chat_request.validate(Some(request_id), &state.chat_validation_limits)?;

    if stream {
        return stream_response(state, chat_request, request_id, started).await;
    }

    let metrics_snapshot = state.metrics.lock().ok().map(|metrics| metrics.snapshot());
    match fallback::complete_chat(
        &state.providers,
        state.resilience.as_ref(),
        chat_request,
        state.routing_policy,
        metrics_snapshot.as_ref(),
    )
    .await
    {
        Ok(success) => {
            let first_choice = success.response.choices.into_iter().next();
            let output_text = first_choice
                .map(|choice| choice.message.content)
                .unwrap_or_default();
            let response = response_object(
                request_id,
                success.response.created,
                success.response.model,
                output_text,
                success.response.usage,
            );
            Ok(Json(response).into_response())
        }
        Err(FallbackError::NoProviderAvailable) => Err(AppError::NoProviderAvailable {
            request_id: Some(request_id),
        }),
        Err(FallbackError::ProviderFailed { error, .. }) => {
            Err(AppError::from_provider_error(error, Some(request_id)))
        }
    }
}

async fn stream_response(
    state: AppState,
    chat_request: crate::models::chat::ChatCompletionRequest,
    request_id: Uuid,
    _started: Instant,
) -> Result<Response, AppError> {
    let metrics_snapshot = state.metrics.lock().ok().map(|metrics| metrics.snapshot());
    let success = match fallback::complete_chat_stream(
        &state.providers,
        state.resilience.as_ref(),
        chat_request,
        state.routing_policy,
        metrics_snapshot.as_ref(),
    )
    .await
    {
        Ok(success) => success,
        Err(FallbackError::NoProviderAvailable) => {
            return Err(AppError::NoProviderAvailable {
                request_id: Some(request_id),
            });
        }
        Err(FallbackError::ProviderFailed { error, .. }) => {
            return Err(AppError::from_provider_error(error, Some(request_id)));
        }
    };

    let mut upstream = success.stream;
    let mut first_event = Some(success.first_event);
    let model = success.context.model.clone();
    let created = success.context.created;
    let event_stream = async_stream::stream! {
        let mut output_text = String::new();
        let mut usage = TokenUsage::default();

        loop {
            let next_item = if let Some(first) = first_event.take() {
                Some(Ok(first))
            } else {
                upstream.next().await
            };

            match next_item {
                Some(Ok(ProviderStreamEvent::Chunk(chunk))) => {
                    for choice in chunk.choices {
                        if let Some(delta) = choice.delta.content {
                            if output_text.chars().count() + delta.chars().count() > MAX_STREAM_OUTPUT_CHARS {
                                let app_error = AppError::ProviderFailure {
                                    request_id: Some(request_id),
                                };
                                let payload = json!({
                                    "type": "error",
                                    "error": {
                                        "message": app_error.public_message(),
                                        "type": "server_error",
                                        "code": "provider_failure",
                                    }
                                });
                                yield Ok::<Event, Infallible>(Event::default().event("error").data(payload.to_string()));
                                return;
                            }
                            output_text.push_str(&delta);
                            let item_id = openai_id("msg", request_id);
                            let payload = ResponseStreamEvent {
                                kind: "response.output_text.delta",
                                payload: ResponseTextDelta {
                                    item_id,
                                    output_index: 0,
                                    content_index: 0,
                                    delta,
                                },
                            };
                            yield json_event("response.output_text.delta", &payload);
                        }
                    }
                }
                Some(Ok(ProviderStreamEvent::Completed { usage: completed_usage })) => {
                    usage = completed_usage;
                    break;
                }
                Some(Err(error)) => {
                    let app_error = AppError::from_provider_error(error, Some(request_id));
                    let payload = json!({
                        "type": "error",
                        "error": {
                            "message": app_error.public_message(),
                            "type": "server_error",
                            "code": "provider_failure",
                        }
                    });
                    yield Ok::<Event, Infallible>(Event::default().event("error").data(payload.to_string()));
                    return;
                }
                None => break,
            }
        }

        let response = response_object(request_id, created, model, output_text, usage);
        let payload = ResponseStreamEvent {
            kind: "response.completed",
            payload: ResponseCompleted { response },
        };
        yield json_event("response.completed", &payload);
        yield Ok::<Event, Infallible>(Event::default().data("[DONE]"));
    };

    Ok(Sse::new(event_stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

pub async fn embeddings(Json(request): Json<Value>) -> Json<Value> {
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("text-embedding-3-small");
    let input_count = match request.get("input") {
        Some(Value::Array(items)) => items.len().max(1),
        Some(_) => 1,
        None => 1,
    };
    let data = (0..input_count)
        .map(|index| {
            json!({
                "object": "embedding",
                "index": index,
                "embedding": deterministic_embedding(index),
            })
        })
        .collect::<Vec<_>>();

    Json(json!({
        "object": "list",
        "data": data,
        "model": model,
        "usage": {
            "prompt_tokens": input_count as u32,
            "total_tokens": input_count as u32
        }
    }))
}

pub async fn moderations(Json(_request): Json<Value>) -> Json<Value> {
    Json(json!({
        "id": openai_id("modr", Uuid::new_v4()),
        "model": "omni-moderation-latest",
        "results": [{
            "flagged": false,
            "categories": {},
            "category_scores": {}
        }]
    }))
}

pub async fn image_generation(Json(request): Json<Value>) -> Json<Value> {
    let n = request
        .get("n")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 10);
    image_response(n as usize)
}

pub async fn image_edit(_body: Bytes) -> Json<Value> {
    image_response(1)
}

pub async fn image_variation(_body: Bytes) -> Json<Value> {
    image_response(1)
}

pub async fn audio_transcription(_body: Bytes) -> Json<Value> {
    Json(json!({ "text": "" }))
}

pub async fn audio_translation(_body: Bytes) -> Json<Value> {
    Json(json!({ "text": "" }))
}

pub async fn list_files() -> Json<Value> {
    Json(json!({ "object": "list", "data": [] }))
}

pub async fn create_file(_body: Bytes) -> Json<Value> {
    Json(file_object(
        openai_id("file", Uuid::new_v4()),
        "uploaded-file",
    ))
}

pub async fn retrieve_file(Path(file_id): Path<String>) -> Json<Value> {
    Json(file_object(file_id, "uploaded-file"))
}

pub async fn delete_file(Path(file_id): Path<String>) -> Json<Value> {
    Json(json!({
        "id": file_id,
        "object": "file",
        "deleted": true
    }))
}

pub async fn file_content(Path(_file_id): Path<String>) -> &'static str {
    ""
}

pub async fn list_batches() -> Json<Value> {
    Json(json!({ "object": "list", "data": [] }))
}

pub async fn create_batch(Json(request): Json<Value>) -> Json<Value> {
    let endpoint = request
        .get("endpoint")
        .and_then(Value::as_str)
        .unwrap_or("/v1/responses");
    Json(batch_object(
        openai_id("batch", Uuid::new_v4()),
        endpoint,
        "validating",
    ))
}

pub async fn retrieve_batch(Path(batch_id): Path<String>) -> Json<Value> {
    Json(batch_object(batch_id, "/v1/responses", "completed"))
}

pub async fn cancel_batch(Path(batch_id): Path<String>) -> Json<Value> {
    Json(batch_object(batch_id, "/v1/responses", "cancelled"))
}

pub async fn list_fine_tuning_jobs() -> Json<Value> {
    Json(json!({ "object": "list", "data": [] }))
}

pub async fn create_fine_tuning_job(Json(request): Json<Value>) -> Json<Value> {
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Json(fine_tuning_job_object(
        openai_id("ftjob", Uuid::new_v4()),
        model,
        "validating_files",
    ))
}

pub async fn retrieve_fine_tuning_job(Path(job_id): Path<String>) -> Json<Value> {
    Json(fine_tuning_job_object(job_id, "unknown", "succeeded"))
}

pub async fn cancel_fine_tuning_job(Path(job_id): Path<String>) -> Json<Value> {
    Json(fine_tuning_job_object(job_id, "unknown", "cancelled"))
}

pub async fn list_fine_tuning_events(Path(_job_id): Path<String>) -> Json<Value> {
    Json(json!({ "object": "list", "data": [] }))
}

pub async fn realtime_session(Json(request): Json<Value>) -> Json<Value> {
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("gpt-4o-realtime-preview");
    Json(json!({
        "id": openai_id("sess", Uuid::new_v4()),
        "object": "realtime.session",
        "model": model,
        "client_secret": {
            "value": openai_id("rt", Uuid::new_v4()),
            "expires_at": unix_timestamp() + 60
        }
    }))
}

fn response_object(
    request_id: Uuid,
    created: i64,
    model: String,
    output_text: String,
    usage: TokenUsage,
) -> ResponseObject {
    ResponseObject {
        id: openai_id("resp", request_id),
        object: "response",
        created_at: created,
        status: "completed",
        model,
        output: vec![ResponseOutput {
            id: openai_id("msg", request_id),
            kind: "message",
            status: "completed",
            role: "assistant",
            content: vec![ResponseOutputContent {
                kind: "output_text",
                text: output_text,
                annotations: Vec::new(),
            }],
        }],
        usage: ResponseUsage::from(usage),
    }
}

fn json_event<T: Serialize>(event: &'static str, payload: &T) -> Result<Event, Infallible> {
    let data = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    Ok(Event::default().event(event).data(data))
}

fn deterministic_embedding(index: usize) -> Vec<f32> {
    (0..16)
        .map(|slot| ((index + slot + 1) as f32) / 100.0)
        .collect()
}

fn image_response(n: usize) -> Json<Value> {
    let data = (0..n)
        .map(|_| {
            json!({
                "url": "data:image/png;base64,",
                "revised_prompt": ""
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "created": unix_timestamp(),
        "data": data
    }))
}

fn file_object(id: String, filename: &str) -> Value {
    json!({
        "id": id,
        "object": "file",
        "bytes": 0,
        "created_at": unix_timestamp(),
        "filename": filename,
        "purpose": "assistants",
        "status": "processed"
    })
}

fn batch_object(id: String, endpoint: &str, status: &str) -> Value {
    json!({
        "id": id,
        "object": "batch",
        "endpoint": endpoint,
        "errors": null,
        "input_file_id": null,
        "completion_window": "24h",
        "status": status,
        "output_file_id": null,
        "error_file_id": null,
        "created_at": unix_timestamp(),
        "in_progress_at": null,
        "expires_at": null,
        "finalizing_at": null,
        "completed_at": null,
        "failed_at": null,
        "expired_at": null,
        "cancelling_at": null,
        "cancelled_at": null,
        "request_counts": {
            "total": 0,
            "completed": 0,
            "failed": 0
        },
        "metadata": {}
    })
}

fn fine_tuning_job_object(id: String, model: &str, status: &str) -> Value {
    json!({
        "id": id,
        "object": "fine_tuning.job",
        "created_at": unix_timestamp(),
        "finished_at": null,
        "model": model,
        "fine_tuned_model": null,
        "organization_id": "rustygate",
        "result_files": [],
        "status": status,
        "validation_file": null,
        "training_file": null,
        "hyperparameters": {},
        "trained_tokens": null,
        "error": null
    })
}
