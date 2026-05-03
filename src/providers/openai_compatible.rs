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

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleProvider {
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub client: Client,
}

#[async_trait]
impl ChatProvider for OpenAiCompatibleProvider {
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
        let endpoint = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let outbound_request = OpenAiChatRequest {
            model: request.model.clone().unwrap_or_else(|| self.model.clone()),
            messages: request.messages.clone(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        let response = self
            .client
            .post(endpoint)
            .bearer_auth(&self.api_key)
            .json(&outbound_request)
            .send()
            .await
            .map_err(map_transport_error)?;
        let status = response.status();

        if !status.is_success() {
            return Err(map_http_status(status));
        }

        let parsed: OpenAiChatResponse = response
            .json()
            .await
            .map_err(|_| ProviderError::ProviderBadResponse)?;
        let first_choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or(ProviderError::ProviderBadResponse)?;
        let completion_text = first_choice.message.content;

        let usage = parsed.usage.unwrap_or_else(|| {
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
            created: parsed.created.unwrap_or(1_700_000_000),
            model: parsed.model,
            provider: self.name.clone(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: ChatRole::Assistant,
                    content: completion_text,
                },
                finish_reason: first_choice
                    .finish_reason
                    .unwrap_or_else(|| "stop".to_string()),
            }],
            usage,
        })
    }
}

#[derive(Debug, Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    model: String,
    #[serde(default)]
    created: Option<i64>,
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<TokenUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiAssistantMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiAssistantMessage {
    content: String,
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
    use super::map_http_status;
    use crate::providers::provider::ProviderError;
    use reqwest::StatusCode;

    #[test]
    fn maps_openai_http_status_to_provider_error() {
        assert!(matches!(
            map_http_status(StatusCode::TOO_MANY_REQUESTS),
            ProviderError::RateLimited
        ));
        assert!(matches!(
            map_http_status(StatusCode::UNAUTHORIZED),
            ProviderError::AuthenticationFailed
        ));
        assert!(matches!(
            map_http_status(StatusCode::INTERNAL_SERVER_ERROR),
            ProviderError::ProviderUnavailable
        ));
    }
}
