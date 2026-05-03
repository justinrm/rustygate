//! Fallback policy scaffolding.
//!
//! Keep fallback logic separate from HTTP handlers so it can be unit tested without binding a
//! server or making network calls.

use std::time::Instant;

use crate::{
    models::chat::{ChatCompletionRequest, ChatCompletionResponse},
    providers::provider::{CostEstimate, ProviderEntry, ProviderError},
    routing::strategy,
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
    request: ChatCompletionRequest,
) -> Result<FallbackSuccess, FallbackError> {
    let candidates = strategy::candidate_providers(providers, request.model.as_deref());

    if candidates.is_empty() {
        return Err(FallbackError::NoProviderAvailable);
    }

    let mut attempts = Vec::new();

    for (index, entry) in candidates.iter().enumerate() {
        let provider_name = entry.provider.name().to_string();
        let started = Instant::now();
        let result = entry.provider.chat_completion(request.clone()).await;
        let latency_ms = started.elapsed().as_millis() as u64;
        let attempt_order = index as u32 + 1;
        let is_fallback = index > 0;

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
                    provider_name,
                    attempt_order,
                    latency_ms,
                    success: false,
                    is_fallback,
                    error_category: Some(provider_error_category(&error)),
                });

                if fallback_decision(&error) == RetryDecision::TryNextProvider
                    && index + 1 < candidates.len()
                {
                    continue;
                }

                return Err(FallbackError::ProviderFailed { error, attempts });
            }
        }
    }

    Err(FallbackError::NoProviderAvailable)
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
