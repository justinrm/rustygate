//! In-memory metrics scaffolding.
//!
//! Keep the MVP simple: aggregate counters, latency samples, provider counts, token estimates, and
//! cost estimates in memory. Add bounded storage before keeping recent request details.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    models::chat::TokenUsage, providers::provider::CostEstimate, routing::fallback::ProviderAttempt,
};

const MAX_LATENCY_SAMPLES: usize = 1_024;
const MAX_ERROR_SAMPLES: usize = 1_024;
const QUEUE_PRESSURE_TOKEN_UNIT: u32 = 1_024;

#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderLoadEstimate {
    pub prompt_tokens: u32,
    pub max_completion_tokens: Option<u32>,
}

impl ProviderLoadEstimate {
    fn queue_pressure_units(self) -> u64 {
        let estimated_tokens = self
            .prompt_tokens
            .saturating_add(self.max_completion_tokens.unwrap_or_default());
        1 + u64::from(estimated_tokens.div_ceil(QUEUE_PRESSURE_TOKEN_UNIT))
    }
}

#[derive(Debug)]
pub struct ProviderInFlightGuard {
    metrics: Arc<Mutex<MetricsRegistry>>,
    provider_name: String,
    queue_pressure_units: u64,
    active: bool,
}

impl ProviderInFlightGuard {
    pub fn new(
        metrics: Arc<Mutex<MetricsRegistry>>,
        provider_name: impl Into<String>,
        load_estimate: ProviderLoadEstimate,
    ) -> Self {
        let provider_name = provider_name.into();
        let queue_pressure_units = load_estimate.queue_pressure_units();
        if let Ok(mut metrics) = metrics.lock() {
            metrics.begin_provider_request(&provider_name, queue_pressure_units);
        }

        Self {
            metrics,
            provider_name,
            queue_pressure_units,
            active: true,
        }
    }

    pub fn finish(&mut self) {
        if self.active {
            if let Ok(mut metrics) = self.metrics.lock() {
                metrics.end_provider_request(&self.provider_name, self.queue_pressure_units);
            }
            self.active = false;
        }
    }
}

impl Drop for ProviderInFlightGuard {
    fn drop(&mut self) {
        self.finish();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOutcome {
    Completed,
    MidStreamFailure,
    IdleTimeout,
    Incomplete,
    Cancelled,
}

impl StreamOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::MidStreamFailure => "mid_stream_failure",
            Self::IdleTimeout => "idle_timeout",
            Self::Incomplete => "incomplete",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug)]
pub struct StreamMetricsGuard {
    metrics: Arc<Mutex<MetricsRegistry>>,
    started: Instant,
    active: bool,
}

impl StreamMetricsGuard {
    pub fn new(metrics: Arc<Mutex<MetricsRegistry>>) -> Self {
        Self {
            metrics,
            started: Instant::now(),
            active: true,
        }
    }

    pub fn finish(&mut self, outcome: StreamOutcome) {
        if self.active {
            let duration_ms = self.started.elapsed().as_millis() as u64;
            if let Ok(mut metrics) = self.metrics.lock() {
                metrics.record_stream_outcome(outcome.as_str(), duration_ms);
            }
            self.active = false;
        }
    }
}

impl Drop for StreamMetricsGuard {
    fn drop(&mut self) {
        self.finish(StreamOutcome::Cancelled);
    }
}

#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
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
    pub requests_by_provider: BTreeMap<String, u64>,
    pub successes_by_provider: BTreeMap<String, u64>,
    pub errors_by_provider: BTreeMap<String, u64>,
    pub fallback_attempts_by_provider: BTreeMap<String, u64>,
    pub request_errors_by_category: BTreeMap<String, u64>,
    pub provider_errors_by_provider_and_category: BTreeMap<String, BTreeMap<String, u64>>,
    pub recent_provider_errors_by_provider_and_category: BTreeMap<String, BTreeMap<String, u64>>,
    pub admission_rejections_by_reason: BTreeMap<String, u64>,
    pub avg_latency_ms_by_provider: BTreeMap<String, f64>,
    pub p95_latency_ms_by_provider: BTreeMap<String, f64>,
    pub in_flight_requests_by_provider: BTreeMap<String, u64>,
    pub p50_ttft_ms_by_provider: BTreeMap<String, f64>,
    pub p95_ttft_ms_by_provider: BTreeMap<String, f64>,
    pub queue_pressure_by_provider: BTreeMap<String, f64>,
    pub routing_decisions_by_policy_and_reason: BTreeMap<String, BTreeMap<String, u64>>,
    pub prefix_fingerprints_by_outcome: BTreeMap<String, u64>,
    pub cache_lookups_by_outcome: BTreeMap<String, u64>,
    pub cache_hit_ratio: f64,
    pub stream_outcomes_by_outcome: BTreeMap<String, u64>,
    pub p95_stream_duration_ms: f64,
}

#[derive(Debug, Default)]
pub struct MetricsRegistry {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub in_flight_requests: u64,
    pub total_provider_attempts: u64,
    pub fallback_attempts: u64,
    pub estimated_prompt_tokens: u64,
    pub estimated_completion_tokens: u64,
    pub estimated_total_tokens: u64,
    pub estimated_input_cost_usd: f64,
    pub estimated_output_cost_usd: f64,
    pub estimated_total_cost_usd: f64,
    requests_by_provider: BTreeMap<String, u64>,
    successes_by_provider: BTreeMap<String, u64>,
    errors_by_provider: BTreeMap<String, u64>,
    fallback_attempts_by_provider: BTreeMap<String, u64>,
    request_errors_by_category: BTreeMap<String, u64>,
    provider_errors_by_provider_and_category: BTreeMap<String, BTreeMap<String, u64>>,
    provider_error_samples: BTreeMap<String, VecDeque<String>>,
    admission_rejections_by_reason: BTreeMap<String, u64>,
    provider_latency_samples_ms: BTreeMap<String, Vec<u64>>,
    provider_ttft_samples_ms: BTreeMap<String, Vec<u64>>,
    provider_total_latency_ms: BTreeMap<String, u128>,
    in_flight_requests_by_provider: BTreeMap<String, u64>,
    provider_queue_pressure_units: BTreeMap<String, u64>,
    routing_decisions_by_policy_and_reason: BTreeMap<String, BTreeMap<String, u64>>,
    prefix_fingerprints_by_outcome: BTreeMap<String, u64>,
    latency_samples_ms: Vec<u64>,
    total_latency_ms: u128,
    cache_lookups_by_outcome: BTreeMap<String, u64>,
    stream_outcomes_by_outcome: BTreeMap<String, u64>,
    stream_duration_samples_ms: Vec<u64>,
}

impl MetricsRegistry {
    pub fn begin_request(&mut self) {
        self.in_flight_requests = self.in_flight_requests.saturating_add(1);
    }

    pub fn end_request(&mut self) {
        self.in_flight_requests = self.in_flight_requests.saturating_sub(1);
    }

    pub fn begin_provider_request(&mut self, provider_name: &str, queue_pressure_units: u64) {
        *self
            .in_flight_requests_by_provider
            .entry(provider_name.to_string())
            .or_default() += 1;
        *self
            .provider_queue_pressure_units
            .entry(provider_name.to_string())
            .or_default() += queue_pressure_units;
    }

    pub fn end_provider_request(&mut self, provider_name: &str, queue_pressure_units: u64) {
        decrement_or_remove(&mut self.in_flight_requests_by_provider, provider_name, 1);
        decrement_or_remove(
            &mut self.provider_queue_pressure_units,
            provider_name,
            queue_pressure_units,
        );
    }

    pub fn record_provider_ttft(&mut self, provider_name: &str, ttft_ms: u64) {
        let samples = self
            .provider_ttft_samples_ms
            .entry(provider_name.to_string())
            .or_default();

        if samples.len() == MAX_LATENCY_SAMPLES {
            samples.remove(0);
        }

        samples.push(ttft_ms);
    }

    pub fn record_routing_decision(&mut self, policy: &str, reason: &str) {
        *self
            .routing_decisions_by_policy_and_reason
            .entry(policy.to_string())
            .or_default()
            .entry(reason.to_string())
            .or_default() += 1;
    }

    pub fn record_prefix_fingerprint(&mut self, outcome: &str) {
        *self
            .prefix_fingerprints_by_outcome
            .entry(outcome.to_string())
            .or_default() += 1;
    }

    pub fn record_admission_rejection(&mut self, reason: &str) {
        *self
            .admission_rejections_by_reason
            .entry(reason.to_string())
            .or_default() += 1;
    }

    pub fn record_success(
        &mut self,
        provider_name: &str,
        usage: &TokenUsage,
        cost_estimate: CostEstimate,
        latency_ms: u64,
    ) {
        let attempts = [ProviderAttempt {
            provider_name: provider_name.to_string(),
            attempt_order: 1,
            latency_ms,
            success: true,
            is_fallback: false,
            error_category: None,
        }];

        self.record_chat_success(provider_name, usage, cost_estimate, latency_ms, &attempts);
    }

    pub fn record_failure(&mut self, provider_name: &str, latency_ms: u64) {
        let attempts = [ProviderAttempt {
            provider_name: provider_name.to_string(),
            attempt_order: 1,
            latency_ms,
            success: false,
            is_fallback: false,
            error_category: None,
        }];

        self.record_chat_failure(latency_ms, &attempts);
    }

    pub fn record_cache_lookup(&mut self, outcome: &str) {
        *self
            .cache_lookups_by_outcome
            .entry(outcome.to_string())
            .or_default() += 1;
    }

    pub fn record_stream_outcome(&mut self, outcome: &str, duration_ms: u64) {
        *self
            .stream_outcomes_by_outcome
            .entry(outcome.to_string())
            .or_default() += 1;

        if self.stream_duration_samples_ms.len() == MAX_LATENCY_SAMPLES {
            self.stream_duration_samples_ms.remove(0);
        }
        self.stream_duration_samples_ms.push(duration_ms);
    }

    pub fn record_chat_success(
        &mut self,
        _provider_name: &str,
        usage: &TokenUsage,
        cost_estimate: CostEstimate,
        latency_ms: u64,
        attempts: &[ProviderAttempt],
    ) {
        self.total_requests += 1;
        self.successful_requests += 1;
        self.estimated_prompt_tokens += u64::from(usage.prompt_tokens);
        self.estimated_completion_tokens += u64::from(usage.completion_tokens);
        self.estimated_total_tokens += u64::from(usage.total_tokens);
        self.estimated_input_cost_usd += cost_estimate.input_cost_usd;
        self.estimated_output_cost_usd += cost_estimate.output_cost_usd;
        self.estimated_total_cost_usd += cost_estimate.total_cost_usd;
        self.total_latency_ms += u128::from(latency_ms);
        self.record_latency_sample(latency_ms);
        self.record_provider_attempts(attempts);
    }

    pub fn record_chat_failure(&mut self, latency_ms: u64, attempts: &[ProviderAttempt]) {
        self.record_chat_failure_with_category(latency_ms, attempts, None);
    }

    pub fn record_chat_failure_with_category(
        &mut self,
        latency_ms: u64,
        attempts: &[ProviderAttempt],
        error_category: Option<&str>,
    ) {
        self.total_requests += 1;
        self.failed_requests += 1;
        if let Some(error_category) = error_category {
            *self
                .request_errors_by_category
                .entry(error_category.to_string())
                .or_default() += 1;
        }
        self.total_latency_ms += u128::from(latency_ms);
        self.record_latency_sample(latency_ms);
        self.record_provider_attempts(attempts);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            total_requests: self.total_requests,
            successful_requests: self.successful_requests,
            failed_requests: self.failed_requests,
            in_flight_requests: self.in_flight_requests,
            total_provider_attempts: self.total_provider_attempts,
            fallback_attempts: self.fallback_attempts,
            error_rate: self.error_rate(),
            avg_latency_ms: self.avg_latency_ms(),
            p95_latency_ms: self.p95_latency_ms(),
            estimated_prompt_tokens: self.estimated_prompt_tokens,
            estimated_completion_tokens: self.estimated_completion_tokens,
            estimated_total_tokens: self.estimated_total_tokens,
            estimated_input_cost_usd: self.estimated_input_cost_usd,
            estimated_output_cost_usd: self.estimated_output_cost_usd,
            estimated_total_cost_usd: self.estimated_total_cost_usd,
            requests_by_provider: self.requests_by_provider.clone(),
            successes_by_provider: self.successes_by_provider.clone(),
            errors_by_provider: self.errors_by_provider.clone(),
            fallback_attempts_by_provider: self.fallback_attempts_by_provider.clone(),
            request_errors_by_category: self.request_errors_by_category.clone(),
            provider_errors_by_provider_and_category: self
                .provider_errors_by_provider_and_category
                .clone(),
            recent_provider_errors_by_provider_and_category: self
                .recent_provider_errors_by_provider_and_category(),
            admission_rejections_by_reason: self.admission_rejections_by_reason.clone(),
            avg_latency_ms_by_provider: self.avg_latency_ms_by_provider(),
            p95_latency_ms_by_provider: self.p95_latency_ms_by_provider(),
            in_flight_requests_by_provider: self.in_flight_requests_by_provider.clone(),
            p50_ttft_ms_by_provider: self.p50_ttft_ms_by_provider(),
            p95_ttft_ms_by_provider: self.p95_ttft_ms_by_provider(),
            queue_pressure_by_provider: self.queue_pressure_by_provider(),
            routing_decisions_by_policy_and_reason: self
                .routing_decisions_by_policy_and_reason
                .clone(),
            prefix_fingerprints_by_outcome: self.prefix_fingerprints_by_outcome.clone(),
            cache_lookups_by_outcome: self.cache_lookups_by_outcome.clone(),
            cache_hit_ratio: self.cache_hit_ratio(),
            stream_outcomes_by_outcome: self.stream_outcomes_by_outcome.clone(),
            p95_stream_duration_ms: self.p95_stream_duration_ms(),
        }
    }

    fn record_provider_attempts(&mut self, attempts: &[ProviderAttempt]) {
        for attempt in attempts {
            self.total_provider_attempts += 1;
            *self
                .requests_by_provider
                .entry(attempt.provider_name.clone())
                .or_default() += 1;
            *self
                .provider_total_latency_ms
                .entry(attempt.provider_name.clone())
                .or_default() += u128::from(attempt.latency_ms);
            self.record_provider_latency_sample(&attempt.provider_name, attempt.latency_ms);

            if attempt.success {
                *self
                    .successes_by_provider
                    .entry(attempt.provider_name.clone())
                    .or_default() += 1;
            } else {
                *self
                    .errors_by_provider
                    .entry(attempt.provider_name.clone())
                    .or_default() += 1;
                if let Some(error_category) = attempt.error_category {
                    self.record_provider_error_sample(
                        &attempt.provider_name,
                        error_category.as_str(),
                    );
                    *self
                        .provider_errors_by_provider_and_category
                        .entry(attempt.provider_name.clone())
                        .or_default()
                        .entry(error_category.as_str().to_string())
                        .or_default() += 1;
                }
            }

            if attempt.is_fallback {
                self.fallback_attempts += 1;
                *self
                    .fallback_attempts_by_provider
                    .entry(attempt.provider_name.clone())
                    .or_default() += 1;
            }
        }
    }

    fn record_latency_sample(&mut self, latency_ms: u64) {
        if self.latency_samples_ms.len() == MAX_LATENCY_SAMPLES {
            self.latency_samples_ms.remove(0);
        }

        self.latency_samples_ms.push(latency_ms);
    }

    fn record_provider_latency_sample(&mut self, provider_name: &str, latency_ms: u64) {
        let samples = self
            .provider_latency_samples_ms
            .entry(provider_name.to_string())
            .or_default();

        if samples.len() == MAX_LATENCY_SAMPLES {
            samples.remove(0);
        }

        samples.push(latency_ms);
    }

    fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.failed_requests as f64 / self.total_requests as f64
        }
    }

    fn avg_latency_ms(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.total_latency_ms as f64 / self.total_requests as f64
        }
    }

    fn p95_latency_ms(&self) -> f64 {
        percentile_latency(&self.latency_samples_ms, 0.95)
    }

    fn avg_latency_ms_by_provider(&self) -> BTreeMap<String, f64> {
        self.requests_by_provider
            .iter()
            .map(|(provider_name, requests)| {
                let total_latency_ms = self
                    .provider_total_latency_ms
                    .get(provider_name)
                    .copied()
                    .unwrap_or_default();
                let avg_latency_ms = if *requests == 0 {
                    0.0
                } else {
                    total_latency_ms as f64 / *requests as f64
                };

                (provider_name.clone(), avg_latency_ms)
            })
            .collect()
    }

    fn p95_latency_ms_by_provider(&self) -> BTreeMap<String, f64> {
        self.provider_latency_samples_ms
            .iter()
            .map(|(provider_name, samples)| {
                (provider_name.clone(), percentile_latency(samples, 0.95))
            })
            .collect()
    }

    fn p95_ttft_ms_by_provider(&self) -> BTreeMap<String, f64> {
        self.provider_ttft_samples_ms
            .iter()
            .map(|(provider_name, samples)| {
                (provider_name.clone(), percentile_latency(samples, 0.95))
            })
            .collect()
    }

    fn p50_ttft_ms_by_provider(&self) -> BTreeMap<String, f64> {
        self.provider_ttft_samples_ms
            .iter()
            .map(|(provider_name, samples)| {
                (provider_name.clone(), percentile_latency(samples, 0.50))
            })
            .collect()
    }

    fn p95_stream_duration_ms(&self) -> f64 {
        percentile_latency(&self.stream_duration_samples_ms, 0.95)
    }

    fn queue_pressure_by_provider(&self) -> BTreeMap<String, f64> {
        self.provider_queue_pressure_units
            .iter()
            .map(|(provider_name, pressure)| (provider_name.clone(), *pressure as f64))
            .collect()
    }

    fn recent_provider_errors_by_provider_and_category(
        &self,
    ) -> BTreeMap<String, BTreeMap<String, u64>> {
        self.provider_error_samples
            .iter()
            .map(|(provider_name, samples)| {
                let mut categories = BTreeMap::new();
                for category in samples {
                    *categories.entry(category.clone()).or_default() += 1;
                }
                (provider_name.clone(), categories)
            })
            .collect()
    }

    fn record_provider_error_sample(&mut self, provider_name: &str, error_category: &str) {
        let samples = self
            .provider_error_samples
            .entry(provider_name.to_string())
            .or_default();
        if samples.len() == MAX_ERROR_SAMPLES {
            samples.pop_front();
        }
        samples.push_back(error_category.to_string());
    }

    fn cache_hit_ratio(&self) -> f64 {
        let hits = self
            .cache_lookups_by_outcome
            .get("hit")
            .copied()
            .unwrap_or_default();
        let misses = self
            .cache_lookups_by_outcome
            .get("miss")
            .copied()
            .unwrap_or_default();
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }
}

fn decrement_or_remove(map: &mut BTreeMap<String, u64>, key: &str, amount: u64) {
    if let Some(value) = map.get_mut(key) {
        *value = value.saturating_sub(amount);
        if *value == 0 {
            map.remove(key);
        }
    }
}

fn percentile_latency(samples: &[u64], percentile: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let percentile_index = ((sorted.len() as f64) * percentile).ceil() as usize - 1;
    sorted[percentile_index] as f64
}

#[cfg(test)]
mod tests {
    use crate::{models::chat::TokenUsage, providers::provider::CostEstimate};

    use super::{MetricsRegistry, ProviderLoadEstimate, MAX_LATENCY_SAMPLES};

    #[test]
    fn snapshot_includes_provider_counts_tokens_and_cost() {
        let mut metrics = MetricsRegistry::default();
        metrics.record_success(
            "mock-fast",
            &TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            },
            CostEstimate {
                input_cost_usd: 0.01,
                output_cost_usd: 0.04,
                total_cost_usd: 0.05,
            },
            40,
        );
        metrics.record_failure("mock-fast", 80);

        let snapshot = metrics.snapshot();

        assert_eq!(snapshot.total_requests, 2);
        assert_eq!(snapshot.successful_requests, 1);
        assert_eq!(snapshot.failed_requests, 1);
        assert_eq!(snapshot.total_provider_attempts, 2);
        assert_eq!(snapshot.fallback_attempts, 0);
        assert_eq!(snapshot.estimated_prompt_tokens, 10);
        assert_eq!(snapshot.estimated_completion_tokens, 20);
        assert_eq!(snapshot.estimated_total_tokens, 30);
        assert!((snapshot.estimated_input_cost_usd - 0.01).abs() < f64::EPSILON);
        assert!((snapshot.estimated_output_cost_usd - 0.04).abs() < f64::EPSILON);
        assert!((snapshot.estimated_total_cost_usd - 0.05).abs() < f64::EPSILON);
        assert_eq!(snapshot.requests_by_provider.get("mock-fast"), Some(&2));
        assert_eq!(snapshot.successes_by_provider.get("mock-fast"), Some(&1));
        assert_eq!(snapshot.errors_by_provider.get("mock-fast"), Some(&1));
        assert_eq!(
            snapshot.avg_latency_ms_by_provider.get("mock-fast"),
            Some(&60.0)
        );
        assert_eq!(
            snapshot.p95_latency_ms_by_provider.get("mock-fast"),
            Some(&80.0)
        );
    }

    #[test]
    fn latency_samples_are_capped_to_recent_values() {
        let mut metrics = MetricsRegistry::default();

        for latency_ms in 0..(MAX_LATENCY_SAMPLES as u64 + 10) {
            metrics.record_failure("mock-fast", latency_ms);
        }

        assert_eq!(metrics.latency_samples_ms.len(), MAX_LATENCY_SAMPLES);
        assert_eq!(metrics.latency_samples_ms.first(), Some(&10));
        assert_eq!(
            metrics.latency_samples_ms.last(),
            Some(&(MAX_LATENCY_SAMPLES as u64 + 9))
        );
    }

    #[test]
    fn p95_latency_uses_retained_samples() {
        let mut metrics = MetricsRegistry::default();

        metrics.record_failure("mock-fast", 10);
        metrics.record_failure("mock-fast", 20);
        metrics.record_failure("mock-fast", 30);
        metrics.record_failure("mock-fast", 40);
        metrics.record_failure("mock-fast", 50);

        assert_eq!(metrics.snapshot().p95_latency_ms, 50.0);
    }

    #[test]
    fn provider_in_flight_and_queue_pressure_release() {
        let mut metrics = MetricsRegistry::default();
        let load_estimate = ProviderLoadEstimate {
            prompt_tokens: 2_100,
            max_completion_tokens: Some(50),
        };
        let pressure_units = load_estimate.queue_pressure_units();

        metrics.begin_provider_request("replica-a", pressure_units);
        let snapshot = metrics.snapshot();

        assert_eq!(
            snapshot.in_flight_requests_by_provider.get("replica-a"),
            Some(&1)
        );
        assert_eq!(
            snapshot.queue_pressure_by_provider.get("replica-a"),
            Some(&(pressure_units as f64))
        );

        metrics.end_provider_request("replica-a", pressure_units);
        let snapshot = metrics.snapshot();

        assert_eq!(
            snapshot.in_flight_requests_by_provider.get("replica-a"),
            None
        );
        assert_eq!(snapshot.queue_pressure_by_provider.get("replica-a"), None);
    }

    #[test]
    fn ttft_samples_and_routing_decisions_are_snapshotted() {
        let mut metrics = MetricsRegistry::default();

        metrics.record_provider_ttft("replica-a", 25);
        metrics.record_provider_ttft("replica-a", 75);
        metrics.record_routing_decision("latency", "recent_latency_p95");
        metrics.record_routing_decision("latency", "recent_latency_p95");

        let snapshot = metrics.snapshot();

        assert_eq!(
            snapshot.p50_ttft_ms_by_provider.get("replica-a"),
            Some(&25.0)
        );
        assert_eq!(
            snapshot.p95_ttft_ms_by_provider.get("replica-a"),
            Some(&75.0)
        );
        assert_eq!(
            snapshot.routing_decisions_by_policy_and_reason["latency"]["recent_latency_p95"],
            2
        );
    }

    #[test]
    fn stream_outcomes_and_duration_are_snapshotted() {
        let mut metrics = MetricsRegistry::default();

        metrics.record_stream_outcome("completed", 100);
        metrics.record_stream_outcome("idle_timeout", 250);

        let snapshot = metrics.snapshot();

        assert_eq!(snapshot.stream_outcomes_by_outcome["completed"], 1);
        assert_eq!(snapshot.stream_outcomes_by_outcome["idle_timeout"], 1);
        assert_eq!(snapshot.p95_stream_duration_ms, 250.0);
    }
}
