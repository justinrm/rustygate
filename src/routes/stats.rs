use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, HeaderMap, HeaderValue},
    response::IntoResponse,
    Json,
};
use tracing::warn;

use crate::{
    app::AppState,
    models::stats::{ProviderStatsResponse, StatsResponse},
    telemetry::prometheus::render_prometheus,
};

pub async fn stats(State(state): State<AppState>) -> Json<StatsResponse> {
    let snapshot = stats_snapshot(&state).await;

    Json(StatsResponse {
        total_requests: snapshot.total_requests,
        successful_requests: snapshot.successful_requests,
        failed_requests: snapshot.failed_requests,
        in_flight_requests: snapshot.in_flight_requests,
        total_provider_attempts: snapshot.total_provider_attempts,
        fallback_attempts: snapshot.fallback_attempts,
        error_rate: snapshot.error_rate,
        avg_latency_ms: snapshot.avg_latency_ms,
        p95_latency_ms: snapshot.p95_latency_ms,
        estimated_prompt_tokens: snapshot.estimated_prompt_tokens,
        estimated_completion_tokens: snapshot.estimated_completion_tokens,
        estimated_total_tokens: snapshot.estimated_total_tokens,
        estimated_input_cost_usd: snapshot.estimated_input_cost_usd,
        estimated_output_cost_usd: snapshot.estimated_output_cost_usd,
        estimated_total_cost_usd: snapshot.estimated_total_cost_usd,
        request_errors_by_category: snapshot.request_errors_by_category,
        cache_lookups_by_outcome: snapshot.cache_lookups_by_outcome,
        cache_hit_ratio: snapshot.cache_hit_ratio,
    })
}

pub async fn provider_stats(State(state): State<AppState>) -> Json<ProviderStatsResponse> {
    let snapshot = stats_snapshot(&state).await;

    Json(ProviderStatsResponse {
        requests_by_provider: snapshot.requests_by_provider,
        successes_by_provider: snapshot.successes_by_provider,
        errors_by_provider: snapshot.errors_by_provider,
        fallback_attempts_by_provider: snapshot.fallback_attempts_by_provider,
        provider_errors_by_provider_and_category: snapshot.provider_errors_by_provider_and_category,
        avg_latency_ms_by_provider: snapshot.avg_latency_ms_by_provider,
        p95_latency_ms_by_provider: snapshot.p95_latency_ms_by_provider,
    })
}

pub async fn prometheus_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let snapshot = stats_snapshot(&state).await;
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );

    (headers, render_prometheus(&snapshot))
}

async fn stats_snapshot(state: &AppState) -> crate::telemetry::metrics::MetricsSnapshot {
    let mut snapshot = if let Some(store) = &state.request_log_store {
        match store.stats_snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(error = %error, "failed to load SQLite stats; falling back to memory");
                memory_snapshot(state)
            }
        }
    } else {
        memory_snapshot(state)
    };

    snapshot.in_flight_requests = memory_snapshot(state).in_flight_requests;
    snapshot
}

fn memory_snapshot(state: &AppState) -> crate::telemetry::metrics::MetricsSnapshot {
    state
        .metrics
        .lock()
        .map(|metrics| metrics.snapshot())
        .unwrap_or_default()
}
