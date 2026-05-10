use std::{convert::Infallible, time::Instant};

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
use serde_json::json;
use tracing::{field, info, warn};
use uuid::Uuid;

use crate::{
    app::{AppState, RequestId},
    auth::keys::AuthenticatedKey,
    cache::response::cache_key_for_request,
    compat::openai_id,
    error::AppError,
    models::chat::{ChatCompletionRequest, TokenUsage},
    providers::provider::ProviderStreamEvent,
    routes::shared::{
        admission_invalid_request_message, log_admission_rejection, next_stream_item,
        pool_name_for_request, record_admission_rejection, record_api_key_usage,
        record_cache_lookup, record_request_metadata, resolve_request_model_alias,
        routing_metrics_snapshot, MetricsRequestGuard, StreamItem,
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
    Extension(RequestId(request_id)): Extension<RequestId>,
    request: Result<Json<ChatCompletionRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let started = Instant::now();
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

    if let Err(reason) = state.admission.check_token_budget(&request) {
        let latency_ms = started.elapsed().as_millis() as u64;
        record_admission_rejection(&state, reason);
        record_chat_failure(
            &state,
            latency_ms,
            &[],
            Some(RequestErrorCategory::InvalidRequest),
        );
        log_admission_rejection(&authenticated_key, request_id, &request, reason);
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
        return Err(AppError::InvalidRequest {
            message: admission_invalid_request_message(reason).into(),
            request_id: Some(request_id),
        });
    }

    let admission_guard = match state
        .admission
        .try_acquire_request(pool_name_for_request(&state, &request))
    {
        Ok(guard) => guard,
        Err(rejection) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_admission_rejection(&state, rejection.reason);
            record_chat_failure(
                &state,
                latency_ms,
                &[],
                Some(RequestErrorCategory::AdmissionRejected),
            );
            log_admission_rejection(&authenticated_key, request_id, &request, rejection.reason);
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
                    Some(RequestErrorCategory::AdmissionRejected),
                    &[],
                    state.request_logging,
                ),
            )
            .await;
            return Err(rejection.into_app_error(Some(request_id)));
        }
    };

    if request.stream_enabled() {
        return stream_chat_completions(
            state,
            authenticated_key,
            request,
            request_id,
            started,
            metrics_guard,
            admission_guard,
        )
        .await;
    }

    let _admission_guard = admission_guard;
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
        request.clone(),
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
        Err(FallbackError::AdmissionRejected(rejection)) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_admission_rejection(&state, rejection.reason);
            record_chat_failure(
                &state,
                latency_ms,
                &[],
                Some(RequestErrorCategory::AdmissionRejected),
            );
            log_admission_rejection(&authenticated_key, request_id, &request, rejection.reason);
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
                    Some(RequestErrorCategory::AdmissionRejected),
                    &[],
                    state.request_logging,
                ),
            )
            .await;

            Err(rejection.into_app_error(Some(request_id)))
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
    admission_guard: AdmissionGuard,
) -> Result<Response, AppError> {
    let metrics_snapshot = routing_metrics_snapshot(&state);
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
        request.clone(),
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
        Err(FallbackError::AdmissionRejected(rejection)) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_admission_rejection(&state, rejection.reason);
            record_chat_failure(
                &state,
                latency_ms,
                &[],
                Some(RequestErrorCategory::AdmissionRejected),
            );
            log_admission_rejection(&authenticated_key, request_id, &request, rejection.reason);
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
    let provider_in_flight_guard = success.in_flight_guard;
    let provider_admission_guard = success.admission_guard;
    let stream_idle_timeout = state.stream_idle_timeout;

    let event_stream = async_stream::stream! {
        let _metrics_guard = metrics_guard;
        let _admission_guard = admission_guard;
        let _provider_in_flight_guard = provider_in_flight_guard;
        let _provider_admission_guard = provider_admission_guard;
        let mut stream_metrics_guard = StreamMetricsGuard::new(state_for_stream.metrics.clone());
        let mut usage: Option<TokenUsage> = None;
        let mut stream_failed = false;
        let mut stream_error_category: Option<RequestErrorCategory> = None;
        let mut stream_outcome: Option<StreamOutcome> = None;

        loop {
            let next_item = next_stream_item(&mut first_event, &mut upstream, stream_idle_timeout).await;

            match next_item {
                StreamItem::Event(ProviderStreamEvent::Chunk(mut chunk)) => {
                    info!(request_id = %request_id, provider = %provider_name, "stream chunk emitted");
                    chunk.id = openai_id("chatcmpl", request_id);
                    let payload = match serde_json::to_string(&chunk) {
                        Ok(payload) => payload,
                        Err(_) => {
                            stream_failed = true;
                            stream_error_category = Some(RequestErrorCategory::ProviderBadResponse);
                            stream_outcome = Some(StreamOutcome::MidStreamFailure);
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
                StreamItem::Event(ProviderStreamEvent::Completed { usage: completed_usage }) => {
                    info!(request_id = %request_id, provider = %provider_name, "stream completed");
                    usage = Some(completed_usage);
                    yield Ok::<Event, Infallible>(Event::default().data("[DONE]"));
                    break;
                }
                StreamItem::Error(error) => {
                    stream_failed = true;
                    warn!(request_id = %request_id, provider = %provider_name, "stream failed");
                    let provider_category = provider_error_category(&error);
                    stream_error_category = Some(provider_category.into());
                    stream_outcome = Some(StreamOutcome::MidStreamFailure);
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
                StreamItem::IdleTimeout => {
                    stream_failed = true;
                    warn!(request_id = %request_id, provider = %provider_name, idle_timeout_ms = stream_idle_timeout.as_millis() as u64, "stream idle timeout exceeded");
                    stream_error_category = Some(RequestErrorCategory::Timeout);
                    stream_outcome = Some(StreamOutcome::IdleTimeout);
                    let app_error = AppError::from_provider_error(
                        crate::providers::provider::ProviderError::Timeout,
                        Some(request_id),
                    );
                    let payload = stream_error_payload(&provider_name, request_id, &app_error);
                    yield Ok::<Event, Infallible>(Event::default().data(payload));
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
                    let payload = stream_error_payload(&provider_name, request_id, &app_error);
                    yield Ok::<Event, Infallible>(Event::default().data(payload));
                    break;
                }
            }
        }

        let latency_ms = started.elapsed().as_millis() as u64;
        if stream_failed || usage.is_none() {
            let error_category = stream_error_category.unwrap_or(RequestErrorCategory::ProviderBadResponse);
            stream_metrics_guard.finish(stream_outcome.unwrap_or(StreamOutcome::MidStreamFailure));
            record_chat_failure(
                &state_for_stream,
                latency_ms,
                &attempts,
                Some(error_category),
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
                    Some(error_category),
                    &attempts,
                    state_for_stream.request_logging,
                ),
            )
            .await;
        } else if let Some(usage) = usage {
            stream_metrics_guard.finish(StreamOutcome::Completed);
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
