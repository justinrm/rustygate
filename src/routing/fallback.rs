//! Fallback policy scaffolding.
//!
//! Keep fallback logic separate from HTTP handlers so it can be unit tested without binding a
//! server or making network calls.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::time::sleep;

use crate::{
    config::RoutingPolicy,
    models::chat::{ChatCompletionRequest, ChatCompletionResponse},
    providers::provider::{
        CostEstimate, ProviderEntry, ProviderError, ProviderPricing, ProviderStream,
        ProviderStreamContext, ProviderStreamEvent,
    },
    routing::{resilience::ResilienceRegistry, strategy},
    telemetry::metrics::MetricsSnapshot,
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
}

#[derive(Debug, Clone)]
pub enum FallbackError {
    NoProviderAvailable,
    ProviderFailed {
        error: ProviderError,
        attempts: Vec<ProviderAttempt>,
    },
}

pub async fn complete_chat(
    providers: &[ProviderEntry],
    resilience: &ResilienceRegistry,
    request: ChatCompletionRequest,
    routing_policy: RoutingPolicy,
    metrics_snapshot: Option<&MetricsSnapshot>,
) -> Result<FallbackSuccess, FallbackError> {
    let candidates = strategy::candidate_providers(
        providers,
        request.model.as_deref(),
        routing_policy,
        metrics_snapshot,
    );

    if candidates.is_empty() {
        return Err(FallbackError::NoProviderAvailable);
    }

    let mut attempts = Vec::new();
    let mut attempt_order = 0_u32;

    for (index, entry) in candidates.iter().enumerate() {
        let provider_name = entry.provider.name().to_string();
        if !resilience.allow_provider_call(&provider_name) {
            continue;
        }
        let retry_policy = resilience.policy_for(&provider_name).retry;
        let is_fallback = index > 0;
        let mut last_retryable_error = None;

        for retry_attempt in 0..=retry_policy.max_retries {
            let started = Instant::now();
            let result = entry.provider.chat_completion(request.clone()).await;
            let latency_ms = started.elapsed().as_millis() as u64;
            attempt_order = attempt_order.saturating_add(1);

            match result {
                Ok(response) => {
                    attempts.push(ProviderAttempt {
                        provider_name: provider_name.clone(),
                        attempt_order,
                        latency_ms,
                        success: true,
                        is_fallback,
                        error_category: None,
                    });
                    resilience.record_success(&provider_name);
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

    Err(FallbackError::NoProviderAvailable)
}

pub async fn complete_chat_stream(
    providers: &[ProviderEntry],
    resilience: &ResilienceRegistry,
    request: ChatCompletionRequest,
    routing_policy: RoutingPolicy,
    metrics_snapshot: Option<&MetricsSnapshot>,
) -> Result<StreamingFallbackSuccess, FallbackError> {
    let candidates = strategy::candidate_providers(
        providers,
        request.model.as_deref(),
        routing_policy,
        metrics_snapshot,
    );
    if candidates.is_empty() {
        return Err(FallbackError::NoProviderAvailable);
    }

    let mut attempts = Vec::new();
    let mut attempt_order = 0_u32;

    for (index, entry) in candidates.iter().enumerate() {
        let provider_name = entry.provider.name().to_string();
        if !resilience.allow_provider_call(&provider_name) {
            continue;
        }
        let retry_policy = resilience.policy_for(&provider_name).retry;
        let is_fallback = index > 0;
        let mut last_retryable_error = None;

        for retry_attempt in 0..=retry_policy.max_retries {
            let started = Instant::now();
            let result = entry.provider.chat_completion_stream(request.clone()).await;

            match result {
                Ok((context, mut stream)) => {
                    let first_event_result = stream.next().await;
                    let latency_ms = started.elapsed().as_millis() as u64;
                    attempt_order = attempt_order.saturating_add(1);

                    match first_event_result {
                        Some(Ok(first_event)) => {
                            attempts.push(ProviderAttempt {
                                provider_name: provider_name.clone(),
                                attempt_order,
                                latency_ms,
                                success: true,
                                is_fallback,
                                error_category: None,
                            });
                            resilience.record_success(&provider_name);
                            return Ok(StreamingFallbackSuccess {
                                context,
                                first_event,
                                stream,
                                provider_name,
                                pricing: entry.pricing,
                                attempts,
                            });
                        }
                        Some(Err(error)) => {
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
                    attempt_order = attempt_order.saturating_add(1);
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

    Err(FallbackError::NoProviderAvailable)
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
