use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::BoxStream;
use uuid::Uuid;

use crate::models::chat::{
    ChatCompletionChunkResponse, ChatCompletionRequest, ChatCompletionResponse, TokenUsage,
};

#[derive(Debug, Clone, thiserror::Error)]
pub enum ProviderError {
    #[error("provider timed out")]
    Timeout,
    #[error("provider is rate limited")]
    RateLimited,
    #[error("provider authentication failed")]
    AuthenticationFailed,
    #[error("provider is unavailable")]
    ProviderUnavailable,
    #[error("provider returned an invalid response")]
    ProviderBadResponse,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ProviderStreamError {
    #[error("provider stream failed before first chunk")]
    BeforeFirstChunk(ProviderError),
    #[error("provider stream failed after partial response")]
    MidStream(ProviderError),
}

impl ProviderStreamError {
    pub fn into_provider_error(self) -> ProviderError {
        match self {
            Self::BeforeFirstChunk(error) | Self::MidStream(error) => error,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProviderStreamEvent {
    Chunk(ChatCompletionChunkResponse),
    Completed { usage: TokenUsage },
}

pub type ProviderStream = BoxStream<'static, Result<ProviderStreamEvent, ProviderError>>;

#[derive(Debug, Clone)]
pub struct ProviderStreamContext {
    pub response_id: Uuid,
    pub created: i64,
    pub model: String,
}

#[derive(Clone)]
pub struct ProviderEntry {
    pub priority: u32,
    pub provider: Arc<dyn ChatProvider>,
    pub pricing: ProviderPricing,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderPricing {
    pub cost_per_1k_input_tokens: f64,
    pub cost_per_1k_output_tokens: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CostEstimate {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub total_cost_usd: f64,
}

impl ProviderPricing {
    pub fn estimate_cost(self, prompt_tokens: u32, completion_tokens: u32) -> CostEstimate {
        let input_cost_usd = (f64::from(prompt_tokens) / 1_000.0) * self.cost_per_1k_input_tokens;
        let output_cost_usd =
            (f64::from(completion_tokens) / 1_000.0) * self.cost_per_1k_output_tokens;

        CostEstimate {
            input_cost_usd,
            output_cost_usd,
            total_cost_usd: input_cost_usd + output_cost_usd,
        }
    }

    pub fn estimate_cost_usd(self, prompt_tokens: u32, completion_tokens: u32) -> f64 {
        self.estimate_cost(prompt_tokens, completion_tokens)
            .total_cost_usd
    }
}

#[async_trait]
pub trait ChatProvider: Send + Sync {
    fn name(&self) -> &str;

    fn model(&self) -> &str;

    fn supports_model(&self, model: &str) -> bool;

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError>;

    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<(ProviderStreamContext, ProviderStream), ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::ProviderPricing;

    #[test]
    fn estimate_cost_uses_configured_input_and_output_rates() {
        let pricing = ProviderPricing {
            cost_per_1k_input_tokens: 1.0,
            cost_per_1k_output_tokens: 2.0,
        };

        let cost = pricing.estimate_cost_usd(250, 100);
        assert!((cost - 0.45).abs() < f64::EPSILON);
    }
}
