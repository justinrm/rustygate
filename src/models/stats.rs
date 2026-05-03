use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub struct StatsResponse {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub in_flight_requests: u64,
    pub total_provider_attempts: u64,
    pub fallback_attempts: u64,
    pub error_rate: f64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub estimated_prompt_tokens: u64,
    pub estimated_completion_tokens: u64,
    pub estimated_total_tokens: u64,
    pub estimated_input_cost_usd: f64,
    pub estimated_output_cost_usd: f64,
    pub estimated_total_cost_usd: f64,
    pub request_errors_by_category: BTreeMap<String, u64>,
    pub cache_lookups_by_outcome: BTreeMap<String, u64>,
    pub cache_hit_ratio: f64,
}

#[derive(Debug, Default, Serialize)]
pub struct ProviderStatsResponse {
    pub requests_by_provider: BTreeMap<String, u64>,
    pub successes_by_provider: BTreeMap<String, u64>,
    pub errors_by_provider: BTreeMap<String, u64>,
    pub fallback_attempts_by_provider: BTreeMap<String, u64>,
    pub provider_errors_by_provider_and_category: BTreeMap<String, BTreeMap<String, u64>>,
    pub avg_latency_ms_by_provider: BTreeMap<String, f64>,
    pub p95_latency_ms_by_provider: BTreeMap<String, f64>,
}
