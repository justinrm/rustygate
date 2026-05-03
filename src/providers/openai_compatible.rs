use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    compat::openai_id,
    models::chat::{
        ChatChoice, ChatCompletionChunkResponse, ChatCompletionRequest, ChatCompletionResponse,
        ChatDelta, ChatMessage, ChatRole, TokenUsage,
    },
    providers::{
        provider::{
            ChatProvider, ProviderError, ProviderStream, ProviderStreamContext, ProviderStreamEvent,
        },
        sse::SseDataParser,
    },
    telemetry::token_estimator::{estimate_tokens_for_messages, estimate_tokens_for_text},
};

const MAX_STREAM_OUTPUT_CHARS: usize = 256 * 1024;

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
        let endpoint = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let outbound_request = OpenAiChatRequest {
            model: request.model.clone().unwrap_or_else(|| self.model.clone()),
            messages: request.messages.clone(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: None,
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
            id: openai_id("chatcmpl", Uuid::new_v4()),
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

    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<(ProviderStreamContext, ProviderStream), ProviderError> {
        let endpoint = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let model = request.model.clone().unwrap_or_else(|| self.model.clone());
        let created = OffsetDateTime::now_utc().unix_timestamp();
        let response_id = Uuid::new_v4();
        let prompt_tokens = estimate_tokens_for_messages(&request.messages);
        let outbound_request = OpenAiChatRequest {
            model: model.clone(),
            messages: request.messages.clone(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: Some(true),
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

        let provider_name = self.name.clone();
        let model_for_stream = model.clone();
        let stream = async_stream::try_stream! {
            let mut parser = SseDataParser::default();
            let mut completion_text = String::new();
            let mut usage: Option<TokenUsage> = None;
            let mut bytes_stream = response.bytes_stream();

            while let Some(bytes) = bytes_stream.next().await {
                let chunk = bytes.map_err(map_transport_error)?;
                let text = std::str::from_utf8(&chunk).map_err(|_| ProviderError::ProviderBadResponse)?;
                let events = parser.push_chunk(text)?;

                for event_data in events {
                    if event_data == "[DONE]" {
                        let completed_usage = usage.take().unwrap_or_else(|| {
                            let completion_tokens = estimate_tokens_for_text(&completion_text);
                            TokenUsage {
                                prompt_tokens,
                                completion_tokens,
                                total_tokens: prompt_tokens + completion_tokens,
                            }
                        });
                        yield ProviderStreamEvent::Completed {
                            usage: completed_usage,
                        };
                        return;
                    }

                    let parsed: OpenAiStreamChunk = serde_json::from_str(&event_data)
                        .map_err(|_| ProviderError::ProviderBadResponse)?;
                    if let Some(event_usage) = parsed.usage {
                        usage = Some(event_usage);
                    }
                    let Some(first_choice) = parsed.choices.into_iter().next() else {
                        continue;
                    };
                    let delta = first_choice.delta.unwrap_or_default();
                    if let Some(content) = delta.content.as_deref() {
                        if completion_text.chars().count() + content.chars().count() > MAX_STREAM_OUTPUT_CHARS {
                            Err::<(), _>(ProviderError::ProviderBadResponse)?;
                        }
                        completion_text.push_str(content);
                    }

                    yield ProviderStreamEvent::Chunk(ChatCompletionChunkResponse::from_delta(
                        response_id,
                        parsed.created.unwrap_or(created),
                        parsed.model.unwrap_or_else(|| model_for_stream.clone()),
                        provider_name.clone(),
                        first_choice.index,
                        ChatDelta {
                            role: delta.role,
                            content: delta.content,
                        },
                        first_choice.finish_reason,
                    ));
                }
            }

            Err::<(), _>(ProviderError::ProviderBadResponse)?;
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

#[derive(Debug, Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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

#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    choices: Vec<OpenAiStreamChoice>,
    #[serde(default)]
    usage: Option<TokenUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    delta: Option<OpenAiStreamDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiStreamDelta {
    #[serde(default)]
    role: Option<ChatRole>,
    #[serde(default)]
    content: Option<String>,
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
