use std::collections::BTreeMap;

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
        admission_rejections_by_reason: snapshot.admission_rejections_by_reason,
        routing_decisions_by_policy_and_reason: snapshot.routing_decisions_by_policy_and_reason,
        prefix_fingerprints_by_outcome: snapshot.prefix_fingerprints_by_outcome,
        cache_lookups_by_outcome: snapshot.cache_lookups_by_outcome,
        cache_hit_ratio: snapshot.cache_hit_ratio,
        stream_outcomes_by_outcome: snapshot.stream_outcomes_by_outcome,
        p95_stream_duration_ms: snapshot.p95_stream_duration_ms,
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
        recent_provider_errors_by_provider_and_category: snapshot
            .recent_provider_errors_by_provider_and_category,
        avg_latency_ms_by_provider: snapshot.avg_latency_ms_by_provider,
        p95_latency_ms_by_provider: snapshot.p95_latency_ms_by_provider,
        in_flight_requests_by_provider: snapshot.in_flight_requests_by_provider,
        p50_ttft_ms_by_provider: snapshot.p50_ttft_ms_by_provider,
        p95_ttft_ms_by_provider: snapshot.p95_ttft_ms_by_provider,
        queue_pressure_by_provider: snapshot.queue_pressure_by_provider,
        circuit_state_by_provider: circuit_state_by_provider(&state),
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

    let memory_snapshot = memory_snapshot(state);
    snapshot.in_flight_requests = memory_snapshot.in_flight_requests;
    snapshot.in_flight_requests_by_provider = memory_snapshot.in_flight_requests_by_provider;
    snapshot.p50_ttft_ms_by_provider = memory_snapshot.p50_ttft_ms_by_provider;
    snapshot.p95_ttft_ms_by_provider = memory_snapshot.p95_ttft_ms_by_provider;
    snapshot.queue_pressure_by_provider = memory_snapshot.queue_pressure_by_provider;
    snapshot.admission_rejections_by_reason = memory_snapshot.admission_rejections_by_reason;
    snapshot.routing_decisions_by_policy_and_reason =
        memory_snapshot.routing_decisions_by_policy_and_reason;
    snapshot.prefix_fingerprints_by_outcome = memory_snapshot.prefix_fingerprints_by_outcome;
    snapshot.stream_outcomes_by_outcome = memory_snapshot.stream_outcomes_by_outcome;
    snapshot.p95_stream_duration_ms = memory_snapshot.p95_stream_duration_ms;
    snapshot.recent_provider_errors_by_provider_and_category =
        memory_snapshot.recent_provider_errors_by_provider_and_category;
    snapshot
}

fn memory_snapshot(state: &AppState) -> crate::telemetry::metrics::MetricsSnapshot {
    state
        .metrics
        .lock()
        .map(|metrics| metrics.snapshot())
        .unwrap_or_default()
}

fn circuit_state_by_provider(state: &AppState) -> BTreeMap<String, String> {
    state
        .providers
        .iter()
        .map(|entry| {
            let provider_name = entry.provider.name().to_string();
            let circuit_state = state
                .resilience
                .circuit_state(&provider_name)
                .as_str()
                .to_string();
            (provider_name, circuit_state)
        })
        .collect()
}
