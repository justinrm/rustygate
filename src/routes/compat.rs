use std::{convert::Infallible, time::Instant};

use axum::{
    body::Bytes,
    extract::{Extension, Path, State},
    http::{HeaderName, HeaderValue},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use tracing::{field, info, warn};
use uuid::Uuid;

use crate::{
    app::{AppState, RequestId},
    auth::keys::AuthenticatedKey,
    cache::response::cache_key_for_request,
    compat::{openai_id, unix_timestamp},
    error::AppError,
    models::{
        chat::TokenUsage,
        responses::{
            ResponseCompleted, ResponseObject, ResponseOutput, ResponseOutputContent,
            ResponseRequest, ResponseStreamEvent, ResponseTextDelta, ResponseUsage,
        },
    },
    providers::provider::{ProviderError, ProviderStreamEvent},
    routes::shared::{
        admission_invalid_request_message, log_admission_rejection, next_stream_item,
        pool_name_for_request, record_admission_rejection, record_api_key_usage,
        record_cache_lookup, record_request_metadata, resolve_request_model_alias,
        MetricsRequestGuard, StreamItem,
    },
    routing::{
        admission::AdmissionGuard,
        fallback::{self, provider_error_category, FallbackError},
    },
    telemetry::{
        metrics::{StreamMetricsGuard, StreamOutcome},
        request_log::{RequestErrorCategory, RequestLogEntry, RequestLogStatus},
    },
};

const MAX_STREAM_OUTPUT_CHARS: usize = 256 * 1024;
const RESPONSES_ROUTE: &str = "/v1/responses";
const CACHE_HEADER: HeaderName = HeaderName::from_static("x-rustygate-cache");

#[tracing::instrument(
    skip_all,
    fields(
        route = RESPONSES_ROUTE,
        request_id = field::Empty,
        gen_ai_system = "rustygate",
        gen_ai_request_model = field::Empty
    )
)]
pub async fn responses(
    State(state): State<AppState>,
    Extension(authenticated_key): Extension<AuthenticatedKey>,
    Extension(RequestId(request_id)): Extension<RequestId>,
    Json(request): Json<ResponseRequest>,
) -> Result<Response, AppError> {
    tracing::Span::current().record("request_id", field::display(request_id));
    let started = Instant::now();
    let stream = request.stream_enabled();
    let mut chat_request = request.into_chat_request();
    resolve_request_model_alias(&state, &mut chat_request);
    if let Some(model) = chat_request.model.as_deref() {
        tracing::Span::current().record("gen_ai_request_model", model);
    }
    chat_request.validate(Some(request_id), &state.chat_validation_limits)?;
    if let Err(reason) = state.admission.check_token_budget(&chat_request) {
        record_admission_rejection(&state, reason);
        log_admission_rejection(&authenticated_key, request_id, &chat_request, reason);
        return Err(AppError::InvalidRequest {
            message: admission_invalid_request_message(reason).into(),
            request_id: Some(request_id),
        });
    }

    let admission_guard = match state
        .admission
        .try_acquire_request(pool_name_for_request(&state, &chat_request))
    {
        Ok(guard) => guard,
        Err(rejection) => {
            record_admission_rejection(&state, rejection.reason);
            log_admission_rejection(
                &authenticated_key,
                request_id,
                &chat_request,
                rejection.reason,
            );
            return Err(rejection.into_app_error(Some(request_id)));
        }
    };

    if stream {
        let metrics_guard = MetricsRequestGuard::new(&state);
        return stream_response(
            state,
            authenticated_key,
            chat_request,
            request_id,
            started,
            metrics_guard,
            admission_guard,
        )
        .await;
    }

    let _admission_guard = admission_guard;
    let cache_key = cache_key_for_request(&chat_request);
    if state.response_cache.is_some() && !authenticated_key.cache_enabled {
        record_cache_lookup(&state, "skip_disabled");
    } else if let (Some(cache), Some(cache_key)) = (&state.response_cache, cache_key.as_ref()) {
        let cache_span = tracing::info_span!("cache_lookup", "cache.hit" = false);
        if let Some(cached) = cache.get(cache_key).await {
            cache_span.record("cache.hit", true);
            record_cache_lookup(&state, "hit");
            let first_choice = cached.choices.into_iter().next();
            let output_text = first_choice
                .map(|choice| choice.message.content)
                .unwrap_or_default();
            let response = response_object(
                request_id,
                cached.created,
                cached.model,
                output_text,
                cached.usage,
            );
            let mut response = Json(response).into_response();
            response
                .headers_mut()
                .insert(CACHE_HEADER, HeaderValue::from_static("HIT"));
            return Ok(response);
        }
        record_cache_lookup(&state, "miss");
    } else if state.response_cache.is_some() {
        let outcome = if chat_request.temperature.unwrap_or_default() > 0.0 {
            "skip_temperature"
        } else {
            "skip_tools"
        };
        record_cache_lookup(&state, outcome);
    }

    let metrics_snapshot = state.metrics.lock().ok().map(|metrics| metrics.snapshot());
    match fallback::complete_chat(
        fallback::FallbackContext {
            providers: &state.providers,
            model_pools: state.model_pools.as_ref(),
            resilience: state.resilience.as_ref(),
            routing_policy: state.routing_policy,
            prefix_affinity: &state.prefix_affinity,
            prefix_affinity_index: state.prefix_affinity_index.as_ref(),
            metrics_snapshot: metrics_snapshot.as_ref(),
            metrics: state.metrics.clone(),
            admission: state.admission.clone(),
        },
        chat_request.clone(),
    )
    .await
    {
        Ok(success) => {
            let first_choice = success.response.choices.first().cloned();
            let output_text = first_choice
                .map(|choice| choice.message.content)
                .unwrap_or_default();
            let response = response_object(
                request_id,
                success.response.created,
                success.response.model.clone(),
                output_text,
                success.response.usage.clone(),
            );
            record_api_key_usage(
                &state,
                &authenticated_key,
                u64::from(response.usage.total_tokens),
                success.cost_estimate.total_cost_usd,
            )
            .await;
            if authenticated_key.cache_enabled {
                if let (Some(cache), Some(cache_key)) = (&state.response_cache, cache_key) {
                    cache
                        .put(
                            cache_key,
                            success.response.clone(),
                            state.response_cache_ttl,
                        )
                        .await;
                }
            }
            let mut response = Json(response).into_response();
            response
                .headers_mut()
                .insert(CACHE_HEADER, HeaderValue::from_static("MISS"));
            Ok(response)
        }
        Err(FallbackError::NoProviderAvailable) => Err(AppError::NoProviderAvailable {
            request_id: Some(request_id),
        }),
        Err(FallbackError::AdmissionRejected(rejection)) => {
            record_admission_rejection(&state, rejection.reason);
            log_admission_rejection(
                &authenticated_key,
                request_id,
                &chat_request,
                rejection.reason,
            );
            Err(rejection.into_app_error(Some(request_id)))
        }
        Err(FallbackError::ProviderFailed { error, .. }) => {
            Err(AppError::from_provider_error(error, Some(request_id)))
        }
    }
}

async fn stream_response(
    state: AppState,
    authenticated_key: AuthenticatedKey,
    chat_request: crate::models::chat::ChatCompletionRequest,
    request_id: Uuid,
    started: Instant,
    metrics_guard: MetricsRequestGuard,
    admission_guard: AdmissionGuard,
) -> Result<Response, AppError> {
    let metrics_snapshot = state.metrics.lock().ok().map(|metrics| metrics.snapshot());
    let success = match fallback::complete_chat_stream(
        fallback::FallbackContext {
            providers: &state.providers,
            model_pools: state.model_pools.as_ref(),
            resilience: state.resilience.as_ref(),
            routing_policy: state.routing_policy,
            prefix_affinity: &state.prefix_affinity,
            prefix_affinity_index: state.prefix_affinity_index.as_ref(),
            metrics_snapshot: metrics_snapshot.as_ref(),
            metrics: state.metrics.clone(),
            admission: state.admission.clone(),
        },
        chat_request.clone(),
    )
    .await
    {
        Ok(success) => success,
        Err(FallbackError::NoProviderAvailable) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_response_failure(
                &state,
                latency_ms,
                &[],
                Some(RequestErrorCategory::NoProviderAvailable),
            );
            record_request_metadata(
                &state,
                RequestLogEntry::new(
                    request_id,
                    RESPONSES_ROUTE,
                    Some(&chat_request),
                    None,
                    RequestLogStatus::Failure,
                    latency_ms,
                    None,
                    None,
                    Some(RequestErrorCategory::NoProviderAvailable),
                    &[],
                    state.request_logging,
                ),
            )
            .await;
            return Err(AppError::NoProviderAvailable {
                request_id: Some(request_id),
            });
        }
        Err(FallbackError::AdmissionRejected(rejection)) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_admission_rejection(&state, rejection.reason);
            record_response_failure(
                &state,
                latency_ms,
                &[],
                Some(RequestErrorCategory::AdmissionRejected),
            );
            log_admission_rejection(
                &authenticated_key,
                request_id,
                &chat_request,
                rejection.reason,
            );
            record_request_metadata(
                &state,
                RequestLogEntry::new(
                    request_id,
                    RESPONSES_ROUTE,
                    Some(&chat_request),
                    None,
                    RequestLogStatus::Failure,
                    latency_ms,
                    None,
                    None,
                    Some(RequestErrorCategory::AdmissionRejected),
                    &[],
                    state.request_logging,
                ),
            )
            .await;
            return Err(rejection.into_app_error(Some(request_id)));
        }
        Err(FallbackError::ProviderFailed { error, attempts }) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            let error_category = attempts
                .last()
                .and_then(|attempt| attempt.error_category)
                .map(Into::into);
            record_response_failure(&state, latency_ms, &attempts, error_category);
            record_request_metadata(
                &state,
                RequestLogEntry::new(
                    request_id,
                    RESPONSES_ROUTE,
                    Some(&chat_request),
                    None,
                    RequestLogStatus::Failure,
                    latency_ms,
                    None,
                    None,
                    error_category,
                    &attempts,
                    state.request_logging,
                ),
            )
            .await;
            return Err(AppError::from_provider_error(error, Some(request_id)));
        }
    };

    let state_for_stream = state.clone();
    let request_for_stream = chat_request.clone();
    let mut upstream = success.stream;
    let mut first_event = Some(success.first_event);
    let model = success.context.model.clone();
    let created = success.context.created;
    let pricing = success.pricing;
    let provider_name = success.provider_name.clone();
    let mut attempts = success.attempts.clone();
    let provider_in_flight_guard = success.in_flight_guard;
    let provider_admission_guard = success.admission_guard;
    let stream_idle_timeout = state.stream_idle_timeout;
    let event_stream = async_stream::stream! {
        let _metrics_guard = metrics_guard;
        let _admission_guard = admission_guard;
        let _provider_in_flight_guard = provider_in_flight_guard;
        let _provider_admission_guard = provider_admission_guard;
        let mut stream_metrics_guard = StreamMetricsGuard::new(state_for_stream.metrics.clone());
        let mut output_text = String::new();
        let mut usage: Option<TokenUsage> = None;
        let mut stream_failed = false;
        let mut stream_error_category: Option<RequestErrorCategory> = None;
        let mut stream_outcome: Option<StreamOutcome> = None;

        loop {
            let next_item = next_stream_item(&mut first_event, &mut upstream, stream_idle_timeout).await;

            match next_item {
                StreamItem::Event(ProviderStreamEvent::Chunk(chunk)) => {
                    info!(request_id = %request_id, "responses stream chunk emitted");
                    for choice in chunk.choices {
                        if let Some(delta) = choice.delta.content {
                            if output_text.chars().count() + delta.chars().count() > MAX_STREAM_OUTPUT_CHARS {
                                stream_failed = true;
                                stream_error_category = Some(RequestErrorCategory::ProviderBadResponse);
                                stream_outcome = Some(StreamOutcome::MidStreamFailure);
                                let app_error = AppError::ProviderFailure {
                                    request_id: Some(request_id),
                                };
                                yield response_stream_error_event(&app_error);
                                break;
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
                StreamItem::Event(ProviderStreamEvent::Completed { usage: completed_usage }) => {
                    info!(request_id = %request_id, "responses stream completed");
                    usage = Some(completed_usage);
                    break;
                }
                StreamItem::Error(error) => {
                    stream_failed = true;
                    warn!(request_id = %request_id, "responses stream failed");
                    let provider_category = provider_error_category(&error);
                    stream_error_category = Some(provider_category.into());
                    stream_outcome = Some(StreamOutcome::MidStreamFailure);
                    let app_error = AppError::from_provider_error(error, Some(request_id));
                    yield response_stream_error_event(&app_error);
                    let latency_ms = started.elapsed().as_millis() as u64;
                    if let Some(last_attempt) = attempts.last_mut() {
                        last_attempt.success = false;
                        last_attempt.error_category = Some(provider_category);
                        last_attempt.latency_ms = latency_ms;
                    }
                    break;
                }
                StreamItem::IdleTimeout => {
                    stream_failed = true;
                    warn!(request_id = %request_id, idle_timeout_ms = stream_idle_timeout.as_millis() as u64, "responses stream idle timeout exceeded");
                    stream_error_category = Some(RequestErrorCategory::Timeout);
                    stream_outcome = Some(StreamOutcome::IdleTimeout);
                    let app_error = AppError::from_provider_error(ProviderError::Timeout, Some(request_id));
                    yield response_stream_error_event(&app_error);
                    let latency_ms = started.elapsed().as_millis() as u64;
                    if let Some(last_attempt) = attempts.last_mut() {
                        last_attempt.success = false;
                        last_attempt.error_category = Some(crate::routing::fallback::ProviderErrorCategory::Timeout);
                        last_attempt.latency_ms = latency_ms;
                    }
                    break;
                }
                StreamItem::End => {
                    stream_failed = true;
                    stream_error_category = Some(RequestErrorCategory::ProviderBadResponse);
                    stream_outcome = Some(StreamOutcome::Incomplete);
                    let app_error = AppError::ProviderFailure {
                        request_id: Some(request_id),
                    };
                    yield response_stream_error_event(&app_error);
                    break;
                }
            }

            if stream_failed {
                break;
            }
        }

        let latency_ms = started.elapsed().as_millis() as u64;
        if stream_failed || usage.is_none() {
            let error_category = stream_error_category.unwrap_or(RequestErrorCategory::ProviderBadResponse);
            stream_metrics_guard.finish(stream_outcome.unwrap_or(StreamOutcome::MidStreamFailure));
            record_response_failure(
                &state_for_stream,
                latency_ms,
                &attempts,
                Some(error_category),
            );
            record_request_metadata(
                &state_for_stream,
                RequestLogEntry::new(
                    request_id,
                    RESPONSES_ROUTE,
                    Some(&request_for_stream),
                    Some(provider_name.clone()),
                    RequestLogStatus::Failure,
                    latency_ms,
                    usage.clone(),
                    None,
                    Some(error_category),
                    &attempts,
                    state_for_stream.request_logging,
                ),
            )
            .await;
            return;
        }

        let Some(usage) = usage else {
            return;
        };
        stream_metrics_guard.finish(StreamOutcome::Completed);
        let response = response_object(request_id, created, model, output_text, usage.clone());
        let cost_estimate = pricing.estimate_cost(usage.prompt_tokens, usage.completion_tokens);
        if let Ok(mut metrics) = state_for_stream.metrics.lock() {
            metrics.record_chat_success(
                &provider_name,
                &usage,
                cost_estimate,
                latency_ms,
                &attempts,
            );
        }
        record_request_metadata(
            &state_for_stream,
            RequestLogEntry::new(
                request_id,
                RESPONSES_ROUTE,
                Some(&request_for_stream),
                Some(provider_name.clone()),
                RequestLogStatus::Success,
                latency_ms,
                Some(usage.clone()),
                Some(cost_estimate),
                None,
                &attempts,
                state_for_stream.request_logging,
            ),
        )
        .await;
        record_api_key_usage(
            &state_for_stream,
            &authenticated_key,
            u64::from(usage.total_tokens),
            cost_estimate.total_cost_usd,
        )
        .await;
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

fn response_stream_error_event(error: &AppError) -> Result<Event, Infallible> {
    let payload = json!({
        "type": "error",
        "error": {
            "message": error.public_message(),
            "type": "server_error",
            "code": "provider_failure",
        }
    });
    Ok(Event::default().event("error").data(payload.to_string()))
}

fn record_response_failure(
    state: &AppState,
    latency_ms: u64,
    attempts: &[fallback::ProviderAttempt],
    error_category: Option<RequestErrorCategory>,
) {
    if let Ok(mut metrics) = state.metrics.lock() {
        metrics.record_chat_failure_with_category(
            latency_ms,
            attempts,
            error_category.map(RequestErrorCategory::as_str),
        );
    }
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
