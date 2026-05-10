//! Request log metadata.
//!
//! Request logs store operational metadata by default. Prompt content stays absent unless a
//! local-development-only config flag explicitly enables it.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    models::chat::{ChatCompletionRequest, ChatMessage, TokenUsage},
    providers::provider::CostEstimate,
    routing::fallback::{ProviderAttempt, ProviderErrorCategory},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestLoggingConfig {
    pub enabled: bool,
    pub log_prompt_content: bool,
}

impl Default for RequestLoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            log_prompt_content: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestLogStatus {
    Success,
    Failure,
}

impl RequestLogStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestErrorCategory {
    InvalidRequest,
    AdmissionRejected,
    NoProviderAvailable,
    Timeout,
    RateLimited,
    AuthenticationFailed,
    ProviderUnavailable,
    ProviderBadResponse,
}

impl RequestErrorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::AdmissionRejected => "admission_rejected",
            Self::NoProviderAvailable => "no_provider_available",
            Self::Timeout => "timeout",
            Self::RateLimited => "rate_limited",
            Self::AuthenticationFailed => "authentication_failed",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderBadResponse => "provider_bad_response",
        }
    }
}

impl From<ProviderErrorCategory> for RequestErrorCategory {
    fn from(category: ProviderErrorCategory) -> Self {
        match category {
            ProviderErrorCategory::Timeout => Self::Timeout,
            ProviderErrorCategory::RateLimited => Self::RateLimited,
            ProviderErrorCategory::AuthenticationFailed => Self::AuthenticationFailed,
            ProviderErrorCategory::ProviderUnavailable => Self::ProviderUnavailable,
            ProviderErrorCategory::ProviderBadResponse => Self::ProviderBadResponse,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RequestCostEstimate {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub total_cost_usd: f64,
}

impl From<CostEstimate> for RequestCostEstimate {
    fn from(cost: CostEstimate) -> Self {
        Self {
            input_cost_usd: cost.input_cost_usd,
            output_cost_usd: cost.output_cost_usd,
            total_cost_usd: cost.total_cost_usd,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestProviderAttempt {
    pub provider_name: String,
    pub attempt_order: u32,
    pub latency_ms: u64,
    pub success: bool,
    pub is_fallback: bool,
    pub error_category: Option<RequestErrorCategory>,
}

impl From<&ProviderAttempt> for RequestProviderAttempt {
    fn from(attempt: &ProviderAttempt) -> Self {
        Self {
            provider_name: attempt.provider_name.clone(),
            attempt_order: attempt.attempt_order,
            latency_ms: attempt.latency_ms,
            success: attempt.success,
            is_fallback: attempt.is_fallback,
            error_category: attempt.error_category.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestLogEntry {
    pub request_id: Uuid,
    pub route: String,
    pub created_at_unix_seconds: i64,
    pub requested_model: Option<String>,
    pub stream: bool,
    pub final_provider: Option<String>,
    pub status: RequestLogStatus,
    pub latency_ms: u64,
    pub usage: Option<TokenUsage>,
    pub cost_estimate: Option<RequestCostEstimate>,
    pub error_category: Option<RequestErrorCategory>,
    pub provider_attempts: Vec<RequestProviderAttempt>,
    pub prompt_messages: Option<Vec<ChatMessage>>,
}

impl RequestLogEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request_id: Uuid,
        route: impl Into<String>,
        request: Option<&ChatCompletionRequest>,
        final_provider: Option<String>,
        status: RequestLogStatus,
        latency_ms: u64,
        usage: Option<TokenUsage>,
        cost_estimate: Option<CostEstimate>,
        error_category: Option<RequestErrorCategory>,
        attempts: &[ProviderAttempt],
        config: RequestLoggingConfig,
    ) -> Self {
        Self {
            request_id,
            route: route.into(),
            created_at_unix_seconds: OffsetDateTime::now_utc().unix_timestamp(),
            requested_model: request.and_then(|request| request.model.clone()),
            stream: request
                .map(|request| request.stream_enabled())
                .unwrap_or(false),
            final_provider,
            status,
            latency_ms,
            usage,
            cost_estimate: cost_estimate.map(Into::into),
            error_category,
            provider_attempts: attempts.iter().map(Into::into).collect(),
            prompt_messages: prompt_messages(request, config),
        }
    }

    pub fn fallback_attempt_count(&self) -> usize {
        self.provider_attempts
            .iter()
            .filter(|attempt| attempt.is_fallback)
            .count()
    }
}

fn prompt_messages(
    request: Option<&ChatCompletionRequest>,
    config: RequestLoggingConfig,
) -> Option<Vec<ChatMessage>> {
    if config.log_prompt_content {
        request.map(|request| request.messages.clone())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::chat::ChatRole;

    #[test]
    fn request_log_entry_redacts_prompt_content_by_default() {
        let request = test_request();
        let entry = RequestLogEntry::new(
            Uuid::new_v4(),
            "/v1/chat/completions",
            Some(&request),
            Some("mock".into()),
            RequestLogStatus::Success,
            7,
            Some(TokenUsage::default()),
            None,
            None,
            &[],
            RequestLoggingConfig::default(),
        );

        assert!(entry.prompt_messages.is_none());
        assert_eq!(entry.requested_model.as_deref(), Some("mock-v1"));
    }

    #[test]
    fn request_log_entry_can_include_prompt_content_when_enabled() {
        let request = test_request();
        let entry = RequestLogEntry::new(
            Uuid::new_v4(),
            "/v1/chat/completions",
            Some(&request),
            Some("mock".into()),
            RequestLogStatus::Success,
            7,
            Some(TokenUsage::default()),
            None,
            None,
            &[],
            RequestLoggingConfig {
                enabled: true,
                log_prompt_content: true,
            },
        );

        assert_eq!(entry.prompt_messages.unwrap()[0].content, "secret prompt");
    }

    fn test_request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: Some("mock-v1".into()),
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: "secret prompt".into(),
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            max_tokens: None,
            stream: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
        }
    }
}
