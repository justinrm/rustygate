//! Mock provider scaffolding.
//!
//! The first real implementation should stay deterministic by default and should not require
//! network access, API keys, or external services.

use async_trait::async_trait;
use futures_util::StreamExt;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    compat::openai_id,
    models::chat::{
        ChatChoice, ChatCompletionChunkResponse, ChatCompletionRequest, ChatCompletionResponse,
        ChatDelta, ChatMessage, ChatRole, TokenUsage,
    },
    providers::provider::{
        ChatProvider, ProviderError, ProviderStream, ProviderStreamContext, ProviderStreamEvent,
    },
    telemetry::token_estimator::{estimate_tokens_for_messages, estimate_tokens_for_text},
};

#[derive(Debug, Clone)]
pub struct MockProvider {
    pub name: String,
    pub model: String,
    pub failure_rate: f64,
    pub base_latency_ms: u64,
}

impl MockProvider {
    pub fn new(name: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            model: model.into(),
            failure_rate: 0.0,
            base_latency_ms: 0,
        }
    }
}

#[async_trait]
impl ChatProvider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn supports_model(&self, model: &str) -> bool {
        self.model == model
    }

    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        if self.failure_rate >= 1.0 {
            return Err(ProviderError::ProviderUnavailable);
        }

        let model = request.model.unwrap_or_else(|| self.model.clone());
        let response_content = format!("Deterministic mock response from {}.", self.name);
        let prompt_tokens = estimate_tokens_for_messages(&request.messages);
        let completion_tokens = estimate_tokens_for_text(&response_content);
        let seed = format!(
            "{}|{}|{}|{}",
            self.name, model, prompt_tokens, completion_tokens
        );
        let response_id = deterministic_uuid(&seed);

        Ok(ChatCompletionResponse {
            id: openai_id("chatcmpl", response_id),
            object: "chat.completion",
            created: 1_700_000_000,
            model,
            provider: self.name.clone(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: ChatRole::Assistant,
                    content: response_content,
                },
                finish_reason: "stop".into(),
            }],
            usage: TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            },
        })
    }

    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<(ProviderStreamContext, ProviderStream), ProviderError> {
        if self.failure_rate >= 1.0 {
            return Err(ProviderError::ProviderUnavailable);
        }

        let model = request.model.clone().unwrap_or_else(|| self.model.clone());
        let response_content = format!("Deterministic mock response from {}.", self.name);
        let prompt_tokens = estimate_tokens_for_messages(&request.messages);
        let completion_tokens = estimate_tokens_for_text(&response_content);
        let seed = format!(
            "{}|{}|{}|{}|stream",
            self.name, model, prompt_tokens, completion_tokens
        );
        let response_id = deterministic_uuid(&seed);
        let created = OffsetDateTime::now_utc().unix_timestamp();
        let provider_name = self.name.clone();

        let model_for_stream = model.clone();
        let stream = async_stream::try_stream! {
            let words: Vec<&str> = response_content.split_whitespace().collect();
            for (index, word) in words.iter().enumerate() {
                let mut content = (*word).to_string();
                if index + 1 < words.len() {
                    content.push(' ');
                }
                yield ProviderStreamEvent::Chunk(ChatCompletionChunkResponse::from_delta(
                    response_id,
                    created,
                    model_for_stream.clone(),
                    provider_name.clone(),
                    0,
                    ChatDelta {
                        role: if index == 0 {
                            Some(ChatRole::Assistant)
                        } else {
                            None
                        },
                        content: Some(content),
                    },
                    None,
                ));
            }

            yield ProviderStreamEvent::Completed {
                usage: TokenUsage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens: prompt_tokens + completion_tokens,
                },
            };
        }
        .boxed();

        Ok((
            ProviderStreamContext {
                response_id,
                created,
                model,
            },
            stream,
        ))
    }
}

fn deterministic_uuid(seed: &str) -> Uuid {
    let mut bytes = [0_u8; 16];
    for (index, seed_byte) in seed.as_bytes().iter().enumerate() {
        let slot = index % 16;
        bytes[slot] = bytes[slot]
            .wrapping_mul(31)
            .wrapping_add(*seed_byte)
            .wrapping_add(slot as u8);
    }

    Uuid::from_bytes(bytes)
}
