//! In-memory metrics scaffolding.
//!
//! Keep the MVP simple: aggregate counters, latency samples, provider counts, token estimates, and
//! cost estimates in memory. Add bounded storage before keeping recent request details.

use std::collections::BTreeMap;

use crate::{
    models::chat::TokenUsage, providers::provider::CostEstimate, routing::fallback::ProviderAttempt,
};

const MAX_LATENCY_SAMPLES: usize = 1_024;

#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
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
    pub avg_latency_ms_by_provider: BTreeMap<String, f64>,
    pub p95_latency_ms_by_provider: BTreeMap<String, f64>,
}

#[derive(Debug, Default)]
pub struct MetricsRegistry {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
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
    provider_latency_samples_ms: BTreeMap<String, Vec<u64>>,
    provider_total_latency_ms: BTreeMap<String, u128>,
    latency_samples_ms: Vec<u64>,
    total_latency_ms: u128,
}

impl MetricsRegistry {
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
        self.total_requests += 1;
        self.failed_requests += 1;
        self.total_latency_ms += u128::from(latency_ms);
        self.record_latency_sample(latency_ms);
        self.record_provider_attempts(attempts);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            total_requests: self.total_requests,
            successful_requests: self.successful_requests,
            failed_requests: self.failed_requests,
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
            avg_latency_ms_by_provider: self.avg_latency_ms_by_provider(),
            p95_latency_ms_by_provider: self.p95_latency_ms_by_provider(),
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

    use super::{MetricsRegistry, MAX_LATENCY_SAMPLES};

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
}
