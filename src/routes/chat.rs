use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Instant,
};

use axum::{
    extract::rejection::JsonRejection,
    extract::{Extension, State},
    http::{HeaderName, HeaderValue},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use futures_util::StreamExt;
use serde_json::json;
use tracing::{field, info, warn};
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::keys::AuthenticatedKey,
    cache::response::cache_key_for_request,
    compat::openai_id,
    error::AppError,
    models::chat::{ChatCompletionRequest, TokenUsage},
    providers::provider::ProviderStreamEvent,
    routing::{
        fallback::{self, provider_error_category, FallbackError},
        strategy::resolve_model_alias,
    },
    telemetry::request_log::{RequestErrorCategory, RequestLogEntry, RequestLogStatus},
};

const CHAT_COMPLETIONS_ROUTE: &str = "/v1/chat/completions";
const CACHE_HEADER: HeaderName = HeaderName::from_static("x-rustygate-cache");

#[tracing::instrument(
    skip_all,
    fields(
        route = CHAT_COMPLETIONS_ROUTE,
        request_id = field::Empty,
        gen_ai_system = "rustygate",
        gen_ai_request_model = field::Empty
    )
)]
pub async fn chat_completions(
    State(state): State<AppState>,
    Extension(authenticated_key): Extension<AuthenticatedKey>,
    request: Result<Json<ChatCompletionRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let started = Instant::now();
    let request_id = Uuid::new_v4();
    tracing::Span::current().record("request_id", field::display(request_id));
    let metrics_guard = MetricsRequestGuard::new(&state);
    let Json(mut request) = match request {
        Ok(request) => request,
        Err(_) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_chat_failure(
                &state,
                latency_ms,
                &[],
                Some(RequestErrorCategory::InvalidRequest),
            );
            record_request_metadata(
                &state,
                RequestLogEntry::new(
                    request_id,
                    CHAT_COMPLETIONS_ROUTE,
                    None,
                    None,
                    RequestLogStatus::Failure,
                    latency_ms,
                    None,
                    None,
                    Some(RequestErrorCategory::InvalidRequest),
                    &[],
                    state.request_logging,
                ),
            )
            .await;
            return Err(AppError::InvalidRequest {
                message: "request body must be valid JSON matching the chat completion schema"
                    .into(),
                request_id: Some(request_id),
            });
        }
    };

    if let Err(error) = request.validate(Some(request_id), &state.chat_validation_limits) {
        let latency_ms = started.elapsed().as_millis() as u64;
        record_chat_failure(
            &state,
            latency_ms,
            &[],
            Some(RequestErrorCategory::InvalidRequest),
        );
        record_request_metadata(
            &state,
            RequestLogEntry::new(
                request_id,
                CHAT_COMPLETIONS_ROUTE,
                Some(&request),
                None,
                RequestLogStatus::Failure,
                latency_ms,
                None,
                None,
                Some(RequestErrorCategory::InvalidRequest),
                &[],
                state.request_logging,
            ),
        )
        .await;
        return Err(error);
    }

    resolve_request_model_alias(&state, &mut request);
    if let Some(model) = request.model.as_deref() {
        tracing::Span::current().record("gen_ai_request_model", model);
    }

    if request.stream_enabled() {
        return stream_chat_completions(
            state,
            authenticated_key,
            request,
            request_id,
            started,
            metrics_guard,
        )
        .await;
    }

    let cache_key = cache_key_for_request(&request);
    if state.response_cache.is_some() && !authenticated_key.cache_enabled {
        record_cache_lookup(&state, "skip_disabled");
    } else if let (Some(cache), Some(cache_key)) = (&state.response_cache, cache_key.as_ref()) {
        let cache_span = tracing::info_span!("cache_lookup", "cache.hit" = false);
        if let Some(response) = cache.get(cache_key).await {
            cache_span.record("cache.hit", true);
            record_cache_lookup(&state, "hit");
            let mut response = Json(response).into_response();
            response
                .headers_mut()
                .insert(CACHE_HEADER, HeaderValue::from_static("HIT"));
            return Ok(response);
        }
        record_cache_lookup(&state, "miss");
    } else if state.response_cache.is_some() {
        let outcome = if request.temperature.unwrap_or_default() > 0.0 {
            "skip_temperature"
        } else {
            "skip_tools"
        };
        record_cache_lookup(&state, outcome);
    }

    let metrics_snapshot = routing_metrics_snapshot(&state);
    match fallback::complete_chat(
        &state.providers,
        state.resilience.as_ref(),
        request.clone(),
        state.routing_policy,
        metrics_snapshot.as_ref(),
    )
    .await
    {
        Ok(success) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            if let Ok(mut metrics) = state.metrics.lock() {
                metrics.record_chat_success(
                    &success.provider_name,
                    &success.response.usage,
                    success.cost_estimate,
                    latency_ms,
                    &success.attempts,
                );
            }

            record_request_metadata(
                &state,
                RequestLogEntry::new(
                    request_id,
                    CHAT_COMPLETIONS_ROUTE,
                    Some(&request),
                    Some(success.provider_name.clone()),
                    RequestLogStatus::Success,
                    latency_ms,
                    Some(success.response.usage.clone()),
                    Some(success.cost_estimate),
                    None,
                    &success.attempts,
                    state.request_logging,
                ),
            )
            .await;

            record_api_key_usage(
                &state,
                &authenticated_key,
                u64::from(success.response.usage.total_tokens),
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
            let mut response = Json(success.response).into_response();
            response
                .headers_mut()
                .insert(CACHE_HEADER, HeaderValue::from_static("MISS"));
            Ok(response)
        }
        Err(FallbackError::NoProviderAvailable) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_chat_failure(
                &state,
                latency_ms,
                &[],
                Some(RequestErrorCategory::NoProviderAvailable),
            );
            record_request_metadata(
                &state,
                RequestLogEntry::new(
                    request_id,
                    CHAT_COMPLETIONS_ROUTE,
                    Some(&request),
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

            Err(AppError::NoProviderAvailable {
                request_id: Some(request_id),
            })
        }
        Err(FallbackError::ProviderFailed { error, attempts }) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            let error_category = attempts
                .last()
                .and_then(|attempt| attempt.error_category)
                .map(Into::into);
            record_chat_failure(&state, latency_ms, &attempts, error_category);
            record_request_metadata(
                &state,
                RequestLogEntry::new(
                    request_id,
                    CHAT_COMPLETIONS_ROUTE,
                    Some(&request),
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

            Err(AppError::from_provider_error(error, Some(request_id)))
        }
    }
}

async fn stream_chat_completions(
    state: AppState,
    authenticated_key: AuthenticatedKey,
    request: ChatCompletionRequest,
    request_id: Uuid,
    started: Instant,
    metrics_guard: MetricsRequestGuard,
) -> Result<Response, AppError> {
    let metrics_snapshot = routing_metrics_snapshot(&state);
    let success = match fallback::complete_chat_stream(
        &state.providers,
        state.resilience.as_ref(),
        request.clone(),
        state.routing_policy,
        metrics_snapshot.as_ref(),
    )
    .await
    {
        Ok(success) => success,
        Err(FallbackError::NoProviderAvailable) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_chat_failure(
                &state,
                latency_ms,
                &[],
                Some(RequestErrorCategory::NoProviderAvailable),
            );
            record_request_metadata(
                &state,
                RequestLogEntry::new(
                    request_id,
                    CHAT_COMPLETIONS_ROUTE,
                    Some(&request),
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
        Err(FallbackError::ProviderFailed { error, attempts }) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            let error_category = attempts
                .last()
                .and_then(|attempt| attempt.error_category)
                .map(Into::into);
            record_chat_failure(&state, latency_ms, &attempts, error_category);
            record_request_metadata(
                &state,
                RequestLogEntry::new(
                    request_id,
                    CHAT_COMPLETIONS_ROUTE,
                    Some(&request),
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
    let request_for_stream = request.clone();
    let provider_name = success.provider_name.clone();
    let mut attempts = success.attempts.clone();
    let pricing = success.pricing;
    let mut upstream = success.stream;
    let mut first_event = Some(success.first_event);

    let event_stream = async_stream::stream! {
        let _metrics_guard = metrics_guard;
        let mut usage: Option<TokenUsage> = None;
        let mut stream_failed = false;

        loop {
            let next_item = if let Some(first) = first_event.take() {
                Some(Ok(first))
            } else {
                upstream.next().await
            };

            match next_item {
                Some(Ok(ProviderStreamEvent::Chunk(mut chunk))) => {
                    info!(request_id = %request_id, provider = %provider_name, "stream chunk emitted");
                    chunk.id = openai_id("chatcmpl", request_id);
                    let payload = match serde_json::to_string(&chunk) {
                        Ok(payload) => payload,
                        Err(_) => {
                            stream_failed = true;
                            let error = AppError::ProviderFailure {
                                request_id: Some(request_id),
                            };
                            let payload = stream_error_payload(&provider_name, request_id, &error);
                            yield Ok::<Event, Infallible>(Event::default().data(payload));
                            break;
                        }
                    };
                    yield Ok::<Event, Infallible>(Event::default().data(payload));
                }
                Some(Ok(ProviderStreamEvent::Completed { usage: completed_usage })) => {
                    info!(request_id = %request_id, provider = %provider_name, "stream completed");
                    usage = Some(completed_usage);
                    yield Ok::<Event, Infallible>(Event::default().data("[DONE]"));
                    break;
                }
                Some(Err(error)) => {
                    stream_failed = true;
                    warn!(request_id = %request_id, provider = %provider_name, "stream failed");
                    let provider_category = provider_error_category(&error);
                    let app_error = AppError::from_provider_error(error, Some(request_id));
                    let payload = stream_error_payload(&provider_name, request_id, &app_error);
                    yield Ok::<Event, Infallible>(Event::default().data(payload));
                    let latency_ms = started.elapsed().as_millis() as u64;
                    if let Some(last_attempt) = attempts.last_mut() {
                        last_attempt.success = false;
                        last_attempt.error_category = Some(provider_category);
                        last_attempt.latency_ms = latency_ms;
                    }
                    break;
                }
                None => {
                    stream_failed = true;
                    let app_error = AppError::ProviderFailure {
                        request_id: Some(request_id),
                    };
                    let payload = stream_error_payload(&provider_name, request_id, &app_error);
                    yield Ok::<Event, Infallible>(Event::default().data(payload));
                    break;
                }
            }
        }

        let latency_ms = started.elapsed().as_millis() as u64;
        if stream_failed || usage.is_none() {
            record_chat_failure(
                &state_for_stream,
                latency_ms,
                &attempts,
                Some(RequestErrorCategory::ProviderBadResponse),
            );
            record_request_metadata(
                &state_for_stream,
                RequestLogEntry::new(
                    request_id,
                    CHAT_COMPLETIONS_ROUTE,
                    Some(&request_for_stream),
                    Some(provider_name.clone()),
                    RequestLogStatus::Failure,
                    latency_ms,
                    usage.clone(),
                    None,
                    Some(RequestErrorCategory::ProviderBadResponse),
                    &attempts,
                    state_for_stream.request_logging,
                ),
            )
            .await;
        } else if let Some(usage) = usage {
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
                    CHAT_COMPLETIONS_ROUTE,
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
        }
    };

    Ok(Sse::new(event_stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

async fn record_api_key_usage(
    state: &AppState,
    authenticated_key: &AuthenticatedKey,
    total_tokens: u64,
    total_cost_usd: f64,
) {
    if let Err(error) = state
        .key_store
        .record_usage(&authenticated_key.id, total_tokens, total_cost_usd)
        .await
    {
        warn!(
            api_key_id = authenticated_key.id,
            error = %error,
            "failed to record API key usage"
        );
    }
}

fn record_cache_lookup(state: &AppState, outcome: &str) {
    if let Ok(mut metrics) = state.metrics.lock() {
        metrics.record_cache_lookup(outcome);
    }
}

fn stream_error_payload(_provider_name: &str, request_id: Uuid, error: &AppError) -> String {
    json!({
        "error": {
            "code": "provider_failure",
            "message": error.public_message(),
            "request_id": request_id,
        }
    })
    .to_string()
}

struct MetricsRequestGuard {
    metrics: Arc<Mutex<crate::telemetry::metrics::MetricsRegistry>>,
    active: bool,
}

impl MetricsRequestGuard {
    fn new(state: &AppState) -> Self {
        if let Ok(mut metrics) = state.metrics.lock() {
            metrics.begin_request();
        }

        Self {
            metrics: state.metrics.clone(),
            active: true,
        }
    }
}

impl Drop for MetricsRequestGuard {
    fn drop(&mut self) {
        if self.active {
            if let Ok(mut metrics) = self.metrics.lock() {
                metrics.end_request();
            }
            self.active = false;
        }
    }
}

fn record_chat_failure(
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

fn resolve_request_model_alias(state: &AppState, request: &mut ChatCompletionRequest) {
    if let Some(model) = request.model.as_deref() {
        request.model = Some(resolve_model_alias(&state.model_aliases, model));
    }
}

fn routing_metrics_snapshot(
    state: &AppState,
) -> Option<crate::telemetry::metrics::MetricsSnapshot> {
    state.metrics.lock().ok().map(|metrics| metrics.snapshot())
}

async fn record_request_metadata(state: &AppState, entry: RequestLogEntry) {
    if !state.request_logging.enabled {
        return;
    }

    let provider = entry.final_provider.as_deref().unwrap_or("none");
    let model = entry.requested_model.as_deref().unwrap_or("unknown");
    let error_category = entry
        .error_category
        .map(|category| category.as_str())
        .unwrap_or("none");
    let prompt_tokens = entry
        .usage
        .as_ref()
        .map(|usage| usage.prompt_tokens)
        .unwrap_or_default();
    let completion_tokens = entry
        .usage
        .as_ref()
        .map(|usage| usage.completion_tokens)
        .unwrap_or_default();
    let total_tokens = entry
        .usage
        .as_ref()
        .map(|usage| usage.total_tokens)
        .unwrap_or_default();
    let estimated_total_cost_usd = entry
        .cost_estimate
        .as_ref()
        .map(|cost| cost.total_cost_usd)
        .unwrap_or_default();
    let attempted_providers: Vec<&str> = entry
        .provider_attempts
        .iter()
        .map(|attempt| attempt.provider_name.as_str())
        .collect();

    info!(
        request_id = %entry.request_id,
        route = %entry.route,
        stream = entry.stream,
        provider,
        model,
        status = entry.status.as_str(),
        latency_ms = entry.latency_ms,
        prompt_tokens,
        completion_tokens,
        total_tokens,
        estimated_total_cost_usd,
        provider_attempts = entry.provider_attempts.len(),
        attempted_providers = ?attempted_providers,
        fallback_attempts = entry.fallback_attempt_count(),
        error_category,
        prompt_content_logged = entry.prompt_messages.is_some(),
        "chat request completed"
    );

    if let Some(store) = &state.request_log_store {
        if let Err(error) = store.record_request(&entry).await {
            warn!(
                request_id = %entry.request_id,
                error = %error,
                "failed to persist request metadata"
            );
        }
    }
}
