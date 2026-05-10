//! Fallback policy scaffolding.
//!
//! Keep fallback logic separate from HTTP handlers so it can be unit tested without binding a
//! server or making network calls.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use tokio::time::sleep;
use tracing::{field, Instrument};

use crate::{
    config::{PrefixAffinityConfig, RoutingPolicy},
    models::chat::{ChatCompletionRequest, ChatCompletionResponse},
    providers::provider::{
        CostEstimate, ProviderEntry, ProviderError, ProviderPricing, ProviderStream,
        ProviderStreamContext, ProviderStreamEvent,
    },
    routing::{
        admission::{AdmissionController, AdmissionGuard, AdmissionRejection},
        model_pools::ModelPoolIndex,
        prefix_affinity::PrefixAffinityIndex,
        prefix_fingerprint::{fingerprint_request, PrefixFingerprintResult},
        resilience::ResilienceRegistry,
        strategy,
    },
    telemetry::{
        metrics::{MetricsRegistry, MetricsSnapshot, ProviderInFlightGuard, ProviderLoadEstimate},
        token_estimator::estimate_tokens_for_messages,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    TryNextProvider,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorCategory {
    Timeout,
    RateLimited,
    AuthenticationFailed,
    ProviderUnavailable,
    ProviderBadResponse,
}

impl ProviderErrorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::RateLimited => "rate_limited",
            Self::AuthenticationFailed => "authentication_failed",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderBadResponse => "provider_bad_response",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderAttempt {
    pub provider_name: String,
    pub attempt_order: u32,
    pub latency_ms: u64,
    pub success: bool,
    pub is_fallback: bool,
    pub error_category: Option<ProviderErrorCategory>,
}

#[derive(Debug, Clone)]
pub struct FallbackSuccess {
    pub response: ChatCompletionResponse,
    pub provider_name: String,
    pub cost_estimate: CostEstimate,
    pub estimated_cost_usd: f64,
    pub attempts: Vec<ProviderAttempt>,
}

pub struct StreamingFallbackSuccess {
    pub context: ProviderStreamContext,
    pub first_event: ProviderStreamEvent,
    pub stream: ProviderStream,
    pub provider_name: String,
    pub pricing: ProviderPricing,
    pub attempts: Vec<ProviderAttempt>,
    pub in_flight_guard: ProviderInFlightGuard,
    pub admission_guard: AdmissionGuard,
}

#[derive(Debug, Clone)]
pub enum FallbackError {
    NoProviderAvailable,
    AdmissionRejected(AdmissionRejection),
    ProviderFailed {
        error: ProviderError,
        attempts: Vec<ProviderAttempt>,
    },
}

pub struct FallbackContext<'a> {
    pub providers: &'a [ProviderEntry],
    pub model_pools: &'a ModelPoolIndex,
    pub resilience: &'a ResilienceRegistry,
    pub routing_policy: RoutingPolicy,
    pub prefix_affinity: &'a PrefixAffinityConfig,
    pub prefix_affinity_index: &'a PrefixAffinityIndex,
    pub metrics_snapshot: Option<&'a MetricsSnapshot>,
    pub metrics: Arc<Mutex<MetricsRegistry>>,
    pub admission: Arc<AdmissionController>,
}

pub async fn complete_chat(
    context: FallbackContext<'_>,
    request: ChatCompletionRequest,
) -> Result<FallbackSuccess, FallbackError> {
    let FallbackContext {
        providers,
        model_pools,
        resilience,
        routing_policy,
        prefix_affinity,
        prefix_affinity_index,
        metrics_snapshot,
        metrics,
        admission,
    } = context;
    let prefix_fingerprint = fingerprint_request(&request);
    record_prefix_fingerprint(metrics.as_ref(), &prefix_fingerprint);
    let selection = strategy::candidate_providers_with_affinity(
        providers,
        request.model.as_deref(),
        routing_policy,
        metrics_snapshot,
        model_pools,
        Some(resilience),
        Some(strategy::PrefixAffinityRouting {
            index: prefix_affinity_index,
            config: prefix_affinity,
            fingerprint: &prefix_fingerprint,
        }),
    );
    let candidates = selection.candidates;
    let effective_policy = selection.effective_policy;
    record_routing_decision(metrics.as_ref(), effective_policy, selection.reason);

    if candidates.is_empty() {
        return Err(FallbackError::NoProviderAvailable);
    }

    let mut attempts = Vec::new();
    let mut attempt_order = 0_u32;
    let mut admission_rejection = None;

    for (index, entry) in candidates.iter().enumerate() {
        let provider_name = entry.provider.name().to_string();
        if !resilience.allow_provider_call(&provider_name) {
            record_routing_decision(metrics.as_ref(), effective_policy, "circuit_open");
            continue;
        }
        let provider_admission_guard = match admission.try_acquire_provider(&provider_name) {
            Ok(guard) => guard,
            Err(rejection) => {
                admission_rejection = Some(rejection);
                continue;
            }
        };
        let retry_policy = resilience.policy_for(&provider_name).retry;
        let is_fallback = index > 0;
        if is_fallback {
            record_routing_decision(metrics.as_ref(), effective_policy, "fallback");
        }
        let mut last_retryable_error = None;

        for retry_attempt in 0..=retry_policy.max_retries {
            let next_attempt_order = attempt_order.saturating_add(1);
            let attempt_span = tracing::info_span!(
                "provider_attempt",
                "gen_ai.system" = "llm_provider",
                "gen_ai.request.model" = request.model.as_deref().unwrap_or("unknown"),
                "rustygate.provider.name" = provider_name.as_str(),
                "rustygate.attempt.order" = next_attempt_order,
                "rustygate.attempt.retry" = retry_attempt,
                "rustygate.attempt.is_fallback" = is_fallback,
                "gen_ai.usage.input_tokens" = field::Empty,
                "gen_ai.usage.output_tokens" = field::Empty,
                "rustygate.attempt.latency_ms" = field::Empty,
                "error.type" = field::Empty,
            );
            let started = Instant::now();
            let _in_flight_guard = ProviderInFlightGuard::new(
                metrics.clone(),
                provider_name.clone(),
                provider_load_estimate(&request),
            );
            let _provider_admission_guard = &provider_admission_guard;
            let result = entry
                .provider
                .chat_completion(request_for_provider(&request, entry.provider.model()))
                .instrument(attempt_span.clone())
                .await;
            let latency_ms = started.elapsed().as_millis() as u64;
            attempt_order = next_attempt_order;
            attempt_span.record("rustygate.attempt.latency_ms", latency_ms);

            match result {
                Ok(response) => {
                    attempt_span.record("gen_ai.usage.input_tokens", response.usage.prompt_tokens);
                    attempt_span.record(
                        "gen_ai.usage.output_tokens",
                        response.usage.completion_tokens,
                    );
                    attempts.push(ProviderAttempt {
                        provider_name: provider_name.clone(),
                        attempt_order,
                        latency_ms,
                        success: true,
                        is_fallback,
                        error_category: None,
                    });
                    resilience.record_success(&provider_name);
                    record_routing_decision(metrics.as_ref(), effective_policy, "selected");
                    record_prefix_affinity_selection(
                        prefix_affinity_index,
                        &prefix_fingerprint,
                        &provider_name,
                        effective_policy,
                    );
                    let cost_estimate = entry.pricing.estimate_cost(
                        response.usage.prompt_tokens,
                        response.usage.completion_tokens,
                    );

                    return Ok(FallbackSuccess {
                        response,
                        provider_name,
                        estimated_cost_usd: cost_estimate.total_cost_usd,
                        cost_estimate,
                        attempts,
                    });
                }
                Err(error) => {
                    attempt_span.record("error.type", provider_error_category(&error).as_str());
                    attempts.push(ProviderAttempt {
                        provider_name: provider_name.clone(),
                        attempt_order,
                        latency_ms,
                        success: false,
                        is_fallback,
                        error_category: Some(provider_error_category(&error)),
                    });

                    if fallback_decision(&error) == RetryDecision::TryNextProvider {
                        last_retryable_error = Some(error.clone());
                        if retry_attempt < retry_policy.max_retries {
                            sleep(retry_backoff(
                                retry_policy,
                                retry_attempt + 1,
                                &provider_name,
                            ))
                            .await;
                            continue;
                        }
                        resilience.record_failure(&provider_name);
                        break;
                    }

                    return Err(FallbackError::ProviderFailed { error, attempts });
                }
            }
        }

        if let Some(error) = last_retryable_error {
            if index + 1 < candidates.len() {
                continue;
            }
            return Err(FallbackError::ProviderFailed { error, attempts });
        }
    }

    if let Some(rejection) = admission_rejection {
        Err(FallbackError::AdmissionRejected(rejection))
    } else {
        Err(FallbackError::NoProviderAvailable)
    }
}

pub async fn complete_chat_stream(
    context: FallbackContext<'_>,
    request: ChatCompletionRequest,
) -> Result<StreamingFallbackSuccess, FallbackError> {
    let FallbackContext {
        providers,
        model_pools,
        resilience,
        routing_policy,
        prefix_affinity,
        prefix_affinity_index,
        metrics_snapshot,
        metrics,
        admission,
    } = context;
    let prefix_fingerprint = fingerprint_request(&request);
    record_prefix_fingerprint(metrics.as_ref(), &prefix_fingerprint);
    let selection = strategy::candidate_providers_with_affinity(
        providers,
        request.model.as_deref(),
        routing_policy,
        metrics_snapshot,
        model_pools,
        Some(resilience),
        Some(strategy::PrefixAffinityRouting {
            index: prefix_affinity_index,
            config: prefix_affinity,
            fingerprint: &prefix_fingerprint,
        }),
    );
    let candidates = selection.candidates;
    let effective_policy = selection.effective_policy;
    record_routing_decision(metrics.as_ref(), effective_policy, selection.reason);
    if candidates.is_empty() {
        return Err(FallbackError::NoProviderAvailable);
    }

    let mut attempts = Vec::new();
    let mut attempt_order = 0_u32;
    let mut admission_rejection = None;

    for (index, entry) in candidates.iter().enumerate() {
        let provider_name = entry.provider.name().to_string();
        if !resilience.allow_provider_call(&provider_name) {
            record_routing_decision(metrics.as_ref(), effective_policy, "circuit_open");
            continue;
        }
        let provider_admission_guard = match admission.try_acquire_provider(&provider_name) {
            Ok(guard) => guard,
            Err(rejection) => {
                admission_rejection = Some(rejection);
                continue;
            }
        };
        let retry_policy = resilience.policy_for(&provider_name).retry;
        let is_fallback = index > 0;
        if is_fallback {
            record_routing_decision(metrics.as_ref(), effective_policy, "fallback");
        }
        let mut last_retryable_error = None;

        for retry_attempt in 0..=retry_policy.max_retries {
            let next_attempt_order = attempt_order.saturating_add(1);
            let attempt_span = tracing::info_span!(
                "provider_stream_attempt",
                "gen_ai.system" = "llm_provider",
                "gen_ai.request.model" = request.model.as_deref().unwrap_or("unknown"),
                "rustygate.provider.name" = provider_name.as_str(),
                "rustygate.attempt.order" = next_attempt_order,
                "rustygate.attempt.retry" = retry_attempt,
                "rustygate.attempt.is_fallback" = is_fallback,
                "rustygate.attempt.latency_ms" = field::Empty,
                "error.type" = field::Empty,
            );
            let started = Instant::now();
            let in_flight_guard = ProviderInFlightGuard::new(
                metrics.clone(),
                provider_name.clone(),
                provider_load_estimate(&request),
            );
            let result = entry
                .provider
                .chat_completion_stream(request_for_provider(&request, entry.provider.model()))
                .instrument(attempt_span.clone())
                .await;

            match result {
                Ok((context, mut stream)) => {
                    let first_event_result = stream.next().await;
                    let latency_ms = started.elapsed().as_millis() as u64;
                    attempt_order = next_attempt_order;
                    attempt_span.record("rustygate.attempt.latency_ms", latency_ms);

                    match first_event_result {
                        Some(Ok(first_event)) => {
                            if let Ok(mut metrics) = metrics.lock() {
                                metrics.record_provider_ttft(&provider_name, latency_ms);
                            }
                            attempts.push(ProviderAttempt {
                                provider_name: provider_name.clone(),
                                attempt_order,
                                latency_ms,
                                success: true,
                                is_fallback,
                                error_category: None,
                            });
                            resilience.record_success(&provider_name);
                            record_routing_decision(metrics.as_ref(), effective_policy, "selected");
                            record_prefix_affinity_selection(
                                prefix_affinity_index,
                                &prefix_fingerprint,
                                &provider_name,
                                effective_policy,
                            );
                            return Ok(StreamingFallbackSuccess {
                                context,
                                first_event,
                                stream,
                                provider_name,
                                pricing: entry.pricing,
                                attempts,
                                in_flight_guard,
                                admission_guard: provider_admission_guard,
                            });
                        }
                        Some(Err(error)) => {
                            attempt_span
                                .record("error.type", provider_error_category(&error).as_str());
                            attempts.push(ProviderAttempt {
                                provider_name: provider_name.clone(),
                                attempt_order,
                                latency_ms,
                                success: false,
                                is_fallback,
                                error_category: Some(provider_error_category(&error)),
                            });
                            if fallback_decision(&error) == RetryDecision::TryNextProvider {
                                last_retryable_error = Some(error.clone());
                                if retry_attempt < retry_policy.max_retries {
                                    sleep(retry_backoff(
                                        retry_policy,
                                        retry_attempt + 1,
                                        &provider_name,
                                    ))
                                    .await;
                                    continue;
                                }
                                resilience.record_failure(&provider_name);
                                break;
                            }
                            return Err(FallbackError::ProviderFailed { error, attempts });
                        }
                        None => {
                            let error = ProviderError::ProviderBadResponse;
                            attempt_span
                                .record("error.type", provider_error_category(&error).as_str());
                            attempts.push(ProviderAttempt {
                                provider_name: provider_name.clone(),
                                attempt_order,
                                latency_ms,
                                success: false,
                                is_fallback,
                                error_category: Some(provider_error_category(&error)),
                            });
                            if index + 1 < candidates.len() {
                                break;
                            }
                            return Err(FallbackError::ProviderFailed { error, attempts });
                        }
                    }
                }
                Err(error) => {
                    let latency_ms = started.elapsed().as_millis() as u64;
                    attempt_order = next_attempt_order;
                    attempt_span.record("rustygate.attempt.latency_ms", latency_ms);
                    attempt_span.record("error.type", provider_error_category(&error).as_str());
                    attempts.push(ProviderAttempt {
                        provider_name: provider_name.clone(),
                        attempt_order,
                        latency_ms,
                        success: false,
                        is_fallback,
                        error_category: Some(provider_error_category(&error)),
                    });

                    if fallback_decision(&error) == RetryDecision::TryNextProvider {
                        last_retryable_error = Some(error.clone());
                        if retry_attempt < retry_policy.max_retries {
                            sleep(retry_backoff(
                                retry_policy,
                                retry_attempt + 1,
                                &provider_name,
                            ))
                            .await;
                            continue;
                        }
                        resilience.record_failure(&provider_name);
                        break;
                    }
                    return Err(FallbackError::ProviderFailed { error, attempts });
                }
            }
        }

        if let Some(error) = last_retryable_error {
            if index + 1 < candidates.len() {
                continue;
            }
            return Err(FallbackError::ProviderFailed { error, attempts });
        }
    }

    if let Some(rejection) = admission_rejection {
        Err(FallbackError::AdmissionRejected(rejection))
    } else {
        Err(FallbackError::NoProviderAvailable)
    }
}

fn retry_backoff(
    policy: crate::routing::resilience::RetryPolicy,
    retry_attempt: u32,
    provider_name: &str,
) -> Duration {
    let base = policy
        .initial_backoff_ms
        .saturating_mul(2_u64.saturating_pow(retry_attempt.saturating_sub(1)));
    let capped = base.min(policy.max_backoff_ms.max(policy.initial_backoff_ms));
    let jitter = deterministic_jitter_ms(provider_name, retry_attempt, policy.jitter_ms);
    Duration::from_millis(capped.saturating_add(jitter))
}

fn deterministic_jitter_ms(provider_name: &str, retry_attempt: u32, jitter_ms: u64) -> u64 {
    if jitter_ms == 0 {
        return 0;
    }
    let byte_sum = provider_name
        .bytes()
        .fold(0_u64, |acc, value| acc.saturating_add(value as u64));
    (byte_sum.saturating_add(retry_attempt as u64 * 17)) % (jitter_ms + 1)
}

pub fn fallback_decision(error: &ProviderError) -> RetryDecision {
    match error {
        ProviderError::Timeout
        | ProviderError::RateLimited
        | ProviderError::ProviderUnavailable => RetryDecision::TryNextProvider,
        ProviderError::AuthenticationFailed | ProviderError::ProviderBadResponse => {
            RetryDecision::Stop
        }
    }
}

pub fn provider_error_category(error: &ProviderError) -> ProviderErrorCategory {
    match error {
        ProviderError::Timeout => ProviderErrorCategory::Timeout,
        ProviderError::RateLimited => ProviderErrorCategory::RateLimited,
        ProviderError::AuthenticationFailed => ProviderErrorCategory::AuthenticationFailed,
        ProviderError::ProviderUnavailable => ProviderErrorCategory::ProviderUnavailable,
        ProviderError::ProviderBadResponse => ProviderErrorCategory::ProviderBadResponse,
    }
}

fn request_for_provider(
    request: &ChatCompletionRequest,
    provider_model: &str,
) -> ChatCompletionRequest {
    let mut request = request.clone();
    request.model = Some(provider_model.to_string());
    request
}

fn provider_load_estimate(request: &ChatCompletionRequest) -> ProviderLoadEstimate {
    ProviderLoadEstimate {
        prompt_tokens: estimate_tokens_for_messages(&request.messages),
        max_completion_tokens: request.max_tokens,
    }
}

fn record_routing_decision(
    metrics: &Mutex<MetricsRegistry>,
    policy: RoutingPolicy,
    reason: &'static str,
) {
    if let Ok(mut metrics) = metrics.lock() {
        metrics.record_routing_decision(policy.as_str(), reason);
    }
}

fn record_prefix_fingerprint(
    metrics: &Mutex<MetricsRegistry>,
    fingerprint: &PrefixFingerprintResult,
) {
    let outcome = if fingerprint.is_high_confidence() {
        "hit"
    } else {
        "miss"
    };

    if let Ok(mut metrics) = metrics.lock() {
        metrics.record_prefix_fingerprint(outcome);
    }
}

fn record_prefix_affinity_selection(
    index: &PrefixAffinityIndex,
    fingerprint: &PrefixFingerprintResult,
    provider_name: &str,
    effective_policy: RoutingPolicy,
) {
    if effective_policy == RoutingPolicy::PrefixAffinity && fingerprint.is_high_confidence() {
        if let Some(fingerprint) = fingerprint.fingerprint.as_deref() {
            index.record(fingerprint, provider_name);
        }
    }
}
