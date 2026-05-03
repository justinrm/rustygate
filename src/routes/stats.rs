use axum::{extract::State, Json};
use tracing::warn;

use crate::{
    app::AppState,
    models::stats::{ProviderStatsResponse, StatsResponse},
};

pub async fn stats(State(state): State<AppState>) -> Json<StatsResponse> {
    let snapshot = if let Some(store) = &state.request_log_store {
        match store.stats_snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(error = %error, "failed to load SQLite stats; falling back to memory");
                memory_snapshot(&state)
            }
        }
    } else {
        memory_snapshot(&state)
    };

    Json(StatsResponse {
        total_requests: snapshot.total_requests,
        successful_requests: snapshot.successful_requests,
        failed_requests: snapshot.failed_requests,
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
    })
}

pub async fn provider_stats(State(state): State<AppState>) -> Json<ProviderStatsResponse> {
    let snapshot = if let Some(store) = &state.request_log_store {
        match store.stats_snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                warn!(
                    error = %error,
                    "failed to load SQLite provider stats; falling back to memory"
                );
                memory_snapshot(&state)
            }
        }
    } else {
        memory_snapshot(&state)
    };

    Json(ProviderStatsResponse {
        requests_by_provider: snapshot.requests_by_provider,
        successes_by_provider: snapshot.successes_by_provider,
        errors_by_provider: snapshot.errors_by_provider,
        fallback_attempts_by_provider: snapshot.fallback_attempts_by_provider,
        avg_latency_ms_by_provider: snapshot.avg_latency_ms_by_provider,
        p95_latency_ms_by_provider: snapshot.p95_latency_ms_by_provider,
    })
}

fn memory_snapshot(state: &AppState) -> crate::telemetry::metrics::MetricsSnapshot {
    state
        .metrics
        .lock()
        .map(|metrics| metrics.snapshot())
        .unwrap_or_default()
}
