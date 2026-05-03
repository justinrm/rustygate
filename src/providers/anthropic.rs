use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    models::chat::{
        ChatChoice, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChatRole,
        TokenUsage,
    },
    providers::provider::{ChatProvider, ProviderError},
    telemetry::token_estimator::{estimate_tokens_for_messages, estimate_tokens_for_text},
};

const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub client: Client,
}

#[async_trait]
impl ChatProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn supports_model(&self, model: &str) -> bool {
        self.model == model
    }

    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        let endpoint = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let (system, messages) = split_system_messages(request.messages.clone());
        let outbound_request = AnthropicMessagesRequest {
            model: request.model.clone().unwrap_or_else(|| self.model.clone()),
            max_tokens: request.max_tokens.unwrap_or(512),
            messages,
            system,
            temperature: request.temperature,
        };

        let response = self
            .client
            .post(endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", DEFAULT_ANTHROPIC_VERSION)
            .json(&outbound_request)
            .send()
            .await
            .map_err(map_transport_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(map_http_status(status));
        }

        let parsed: AnthropicMessagesResponse = response
            .json()
            .await
            .map_err(|_| ProviderError::ProviderBadResponse)?;
        let completion_text = parsed
            .content
            .iter()
            .find(|item| item.kind == "text")
            .and_then(|item| item.text.clone())
            .ok_or(ProviderError::ProviderBadResponse)?;

        let usage = parsed.usage.map(TokenUsage::from).unwrap_or_else(|| {
            let prompt_tokens = estimate_tokens_for_messages(&request.messages);
            let completion_tokens = estimate_tokens_for_text(&completion_text);
            TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            }
        });

        Ok(ChatCompletionResponse {
            id: Uuid::new_v4(),
            object: "chat.completion",
            created: 1_700_000_000,
            model: parsed.model,
            provider: self.name.clone(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: ChatRole::Assistant,
                    content: completion_text,
                },
                finish_reason: parsed.stop_reason.unwrap_or_else(|| "stop".to_string()),
            }],
            usage,
        })
    }
}

#[derive(Debug, Serialize)]
struct AnthropicMessagesRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessagesResponse {
    model: String,
    content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

impl From<AnthropicUsage> for TokenUsage {
    fn from(value: AnthropicUsage) -> Self {
        let total_tokens = value.input_tokens + value.output_tokens;
        Self {
            prompt_tokens: value.input_tokens,
            completion_tokens: value.output_tokens,
            total_tokens,
        }
    }
}

fn split_system_messages(messages: Vec<ChatMessage>) -> (Option<String>, Vec<AnthropicMessage>) {
    let mut system_messages = Vec::new();
    let mut converted = Vec::new();

    for message in messages {
        match message.role {
            ChatRole::System => system_messages.push(message.content),
            ChatRole::User => converted.push(AnthropicMessage {
                role: "user",
                content: message.content,
            }),
            ChatRole::Assistant => converted.push(AnthropicMessage {
                role: "assistant",
                content: message.content,
            }),
        }
    }

    let system = if system_messages.is_empty() {
        None
    } else {
        Some(system_messages.join("\n\n"))
    };

    (system, converted)
}

fn map_http_status(status: StatusCode) -> ProviderError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderError::AuthenticationFailed,
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited,
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => ProviderError::Timeout,
        status if status.is_server_error() => ProviderError::ProviderUnavailable,
        _ => ProviderError::ProviderBadResponse,
    }
}

fn map_transport_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::ProviderUnavailable
    }
}

#[cfg(test)]
mod tests {
    use crate::models::chat::{ChatMessage, ChatRole};

    use super::split_system_messages;

    #[test]
    fn system_messages_are_collapsed_for_anthropic_payload() {
        let (system, messages) = split_system_messages(vec![
            ChatMessage {
                role: ChatRole::System,
                content: "First".to_string(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: "Question".to_string(),
            },
            ChatMessage {
                role: ChatRole::System,
                content: "Second".to_string(),
            },
        ]);

        assert_eq!(system.as_deref(), Some("First\n\nSecond"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }
}
