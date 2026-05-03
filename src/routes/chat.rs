use std::time::Instant;

use axum::{extract::rejection::JsonRejection, extract::State, Json};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    app::AppState,
    error::AppError,
    models::chat::{ChatCompletionRequest, ChatCompletionResponse},
    routing::fallback::{self, FallbackError},
    telemetry::request_log::{RequestErrorCategory, RequestLogEntry, RequestLogStatus},
};

const CHAT_COMPLETIONS_ROUTE: &str = "/v1/chat/completions";

pub async fn chat_completions(
    State(state): State<AppState>,
    request: Result<Json<ChatCompletionRequest>, JsonRejection>,
) -> Result<Json<ChatCompletionResponse>, AppError> {
    let started = Instant::now();
    let request_id = Uuid::new_v4();
    let Json(request) = match request {
        Ok(request) => request,
        Err(_) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_chat_failure(&state, latency_ms, &[]);
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

    if let Err(error) = request.validate(Some(request_id)) {
        let latency_ms = started.elapsed().as_millis() as u64;
        record_chat_failure(&state, latency_ms, &[]);
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

    match fallback::complete_chat(&state.providers, request.clone()).await {
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

            Ok(Json(success.response))
        }
        Err(FallbackError::NoProviderAvailable) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            record_chat_failure(&state, latency_ms, &[]);
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
            record_chat_failure(&state, latency_ms, &attempts);
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

fn record_chat_failure(state: &AppState, latency_ms: u64, attempts: &[fallback::ProviderAttempt]) {
    if let Ok(mut metrics) = state.metrics.lock() {
        metrics.record_chat_failure(latency_ms, attempts);
    }
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
