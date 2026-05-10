use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::StreamExt;
use tokio::time::timeout;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::keys::AuthenticatedKey,
    models::chat::ChatCompletionRequest,
    providers::provider::{ProviderError, ProviderStream, ProviderStreamEvent},
    routing::{admission::AdmissionRejectionReason, strategy::resolve_model_alias},
    telemetry::request_log::RequestLogEntry,
};

pub(crate) enum StreamItem {
    Event(ProviderStreamEvent),
    Error(ProviderError),
    IdleTimeout,
    End,
}

pub(crate) async fn next_stream_item(
    first_event: &mut Option<ProviderStreamEvent>,
    upstream: &mut ProviderStream,
    idle_timeout: Duration,
) -> StreamItem {
    if let Some(first) = first_event.take() {
        return StreamItem::Event(first);
    }

    match timeout(idle_timeout, upstream.next()).await {
        Ok(Some(Ok(event))) => StreamItem::Event(event),
        Ok(Some(Err(error))) => StreamItem::Error(error),
        Ok(None) => StreamItem::End,
        Err(_) => StreamItem::IdleTimeout,
    }
}

pub(crate) struct MetricsRequestGuard {
    metrics: Arc<Mutex<crate::telemetry::metrics::MetricsRegistry>>,
    active: bool,
}

impl MetricsRequestGuard {
    pub(crate) fn new(state: &AppState) -> Self {
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

pub(crate) async fn record_api_key_usage(
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

pub(crate) async fn record_request_metadata(state: &AppState, entry: RequestLogEntry) {
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
        "request completed"
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

pub(crate) fn record_cache_lookup(state: &AppState, outcome: &str) {
    if let Ok(mut metrics) = state.metrics.lock() {
        metrics.record_cache_lookup(outcome);
    }
}

pub(crate) fn record_admission_rejection(state: &AppState, reason: AdmissionRejectionReason) {
    if let Ok(mut metrics) = state.metrics.lock() {
        metrics.record_admission_rejection(reason.as_str());
    }
}

pub(crate) fn log_admission_rejection(
    authenticated_key: &AuthenticatedKey,
    request_id: Uuid,
    request: &ChatCompletionRequest,
    reason: AdmissionRejectionReason,
) {
    warn!(
        request_id = %request_id,
        model = request.model.as_deref().unwrap_or("unknown"),
        api_key_id = authenticated_key.id.as_str(),
        admission_rejection_reason = reason.as_str(),
        "request rejected by admission control"
    );
}

pub(crate) fn admission_invalid_request_message(reason: AdmissionRejectionReason) -> &'static str {
    match reason {
        AdmissionRejectionReason::MaxEstimatedPromptTokens => {
            "estimated prompt token limit exceeded"
        }
        AdmissionRejectionReason::MaxEstimatedTotalTokens => "estimated total token limit exceeded",
        AdmissionRejectionReason::GlobalInFlightLimit
        | AdmissionRejectionReason::PoolInFlightLimit
        | AdmissionRejectionReason::ProviderInFlightLimit => {
            "request rejected by admission control"
        }
    }
}

pub(crate) fn resolve_request_model_alias(state: &AppState, request: &mut ChatCompletionRequest) {
    if let Some(model) = request.model.as_deref() {
        request.model = Some(resolve_model_alias(&state.model_aliases, model));
    }
}

pub(crate) fn pool_name_for_request<'a>(
    state: &'a AppState,
    request: &ChatCompletionRequest,
) -> Option<&'a str> {
    request
        .model
        .as_deref()
        .and_then(|model| state.model_pools.pool_for_public_model(model))
        .map(|pool| pool.name.as_str())
}

pub(crate) fn routing_metrics_snapshot(
    state: &AppState,
) -> Option<crate::telemetry::metrics::MetricsSnapshot> {
    state.metrics.lock().ok().map(|metrics| metrics.snapshot())
}
