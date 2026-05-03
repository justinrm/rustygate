use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{header::HeaderMap, Client, StatusCode};
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
        anthropic_tool_translation::{
            anthropic_content_to_openai_message, openai_tool_choice_to_anthropic,
            openai_tools_to_anthropic, AnthropicContentBlock, AnthropicTool, AnthropicToolChoice,
            AnthropicToolStreamTranslator,
        },
        provider::{
            ChatProvider, ProviderError, ProviderStream, ProviderStreamContext, ProviderStreamEvent,
        },
        sse::SseDataParser,
    },
    telemetry::token_estimator::{estimate_tokens_for_messages, estimate_tokens_for_text},
};

const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_STREAM_OUTPUT_CHARS: usize = 256 * 1024;

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
        let endpoint = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let (system, messages) = split_system_messages(request.messages.clone());
        let tools = request.tools.as_deref().map(openai_tools_to_anthropic);
        let tool_choice = request
            .tool_choice
            .as_ref()
            .and_then(openai_tool_choice_to_anthropic);
        let outbound_request = AnthropicMessagesRequest {
            model: request.model.clone().unwrap_or_else(|| self.model.clone()),
            max_tokens: request.max_tokens.unwrap_or(512),
            messages,
            system,
            temperature: request.temperature,
            stream: None,
            tools,
            tool_choice,
        };

        let mut headers = HeaderMap::new();
        crate::telemetry::tracing::inject_trace_context(&mut headers);

        let response = self
            .client
            .post(endpoint)
            .headers(headers)
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
        let (completion_text, tool_calls) = anthropic_content_to_openai_message(&parsed.content);
        if completion_text.is_empty() && tool_calls.is_none() {
            return Err(ProviderError::ProviderBadResponse);
        }

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
            id: openai_id("chatcmpl", Uuid::new_v4()),
            object: "chat.completion".into(),
            created: 1_700_000_000,
            model: parsed.model,
            provider: self.name.clone(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: ChatRole::Assistant,
                    content: completion_text,
                    tool_calls,
                    tool_call_id: None,
                },
                finish_reason: parsed.stop_reason.unwrap_or_else(|| "stop".to_string()),
            }],
            usage,
        })
    }

    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<(ProviderStreamContext, ProviderStream), ProviderError> {
        let endpoint = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let (system, messages) = split_system_messages(request.messages.clone());
        let model = request.model.clone().unwrap_or_else(|| self.model.clone());
        let created = OffsetDateTime::now_utc().unix_timestamp();
        let response_id = Uuid::new_v4();
        let tools = request.tools.as_deref().map(openai_tools_to_anthropic);
        let tool_choice = request
            .tool_choice
            .as_ref()
            .and_then(openai_tool_choice_to_anthropic);
        let outbound_request = AnthropicMessagesRequest {
            model: model.clone(),
            max_tokens: request.max_tokens.unwrap_or(512),
            messages,
            system,
            temperature: request.temperature,
            stream: Some(true),
            tools,
            tool_choice,
        };

        let mut headers = HeaderMap::new();
        crate::telemetry::tracing::inject_trace_context(&mut headers);

        let response = self
            .client
            .post(endpoint)
            .headers(headers)
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

        let provider_name = self.name.clone();
        let model_for_stream = model.clone();
        let stream = async_stream::try_stream! {
            let mut parser = SseDataParser::default();
            let mut bytes_stream = response.bytes_stream();
            let mut completion_text = String::new();
            let mut prompt_tokens = estimate_tokens_for_messages(&request.messages);
            let mut completion_tokens: Option<u32> = None;
            let mut emitted_role = false;
            let mut tool_translator = AnthropicToolStreamTranslator::default();

            while let Some(bytes) = bytes_stream.next().await {
                let chunk = bytes.map_err(map_transport_error)?;
                let text = std::str::from_utf8(&chunk).map_err(|_| ProviderError::ProviderBadResponse)?;
                let events = parser.push_chunk(text)?;

                for event_data in events {
                    let parsed: AnthropicStreamEvent = serde_json::from_str(&event_data)
                        .map_err(|_| ProviderError::ProviderBadResponse)?;

                    match parsed.kind.as_str() {
                        "message_start" => {
                            if let Some(message) = parsed.message {
                                if let Some(message_usage) = message.usage {
                                    prompt_tokens = message_usage.input_tokens;
                                }
                            }
                        }
                        "content_block_delta" => {
                            let Some(delta) = parsed.delta else {
                                continue;
                            };
                            if delta.kind == "input_json_delta" {
                                if let Some(partial_json) = delta.partial_json {
                                    if let Some(delta) = tool_translator.input_json_delta(partial_json) {
                                        yield ProviderStreamEvent::Chunk(ChatCompletionChunkResponse::from_delta(
                                            response_id,
                                            created,
                                            model_for_stream.clone(),
                                            provider_name.clone(),
                                            0,
                                            delta,
                                            None,
                                        ));
                                    }
                                }
                                continue;
                            }
                            if delta.kind != "text_delta" {
                                continue;
                            }
                            let Some(content) = delta.text else {
                                continue;
                            };
                            if completion_text.chars().count() + content.chars().count() > MAX_STREAM_OUTPUT_CHARS {
                                Err::<(), _>(ProviderError::ProviderBadResponse)?;
                            }
                            completion_text.push_str(&content);
                            yield ProviderStreamEvent::Chunk(ChatCompletionChunkResponse::from_delta(
                                response_id,
                                created,
                                model_for_stream.clone(),
                                provider_name.clone(),
                                0,
                                ChatDelta {
                                    role: if emitted_role {
                                        None
                                    } else {
                                        emitted_role = true;
                                        Some(ChatRole::Assistant)
                                    },
                                    content: Some(content),
                                    tool_calls: None,
                                },
                                None,
                            ));
                        }
                        "content_block_start" => {
                            if let (Some(index), Some(block)) = (parsed.index, parsed.content_block.as_ref()) {
                                if let Some(delta) = tool_translator.content_block_start(index, block) {
                                    yield ProviderStreamEvent::Chunk(ChatCompletionChunkResponse::from_delta(
                                        response_id,
                                        created,
                                        model_for_stream.clone(),
                                        provider_name.clone(),
                                        0,
                                        delta,
                                        None,
                                    ));
                                }
                            }
                        }
                        "message_delta" => {
                            if let Some(event_usage) = parsed.usage {
                                completion_tokens = Some(event_usage.output_tokens);
                            }
                            let finish_reason = parsed.delta.and_then(|delta| delta.stop_reason);
                            if finish_reason.is_some() {
                                yield ProviderStreamEvent::Chunk(ChatCompletionChunkResponse::from_delta(
                                    response_id,
                                    created,
                                    model_for_stream.clone(),
                                    provider_name.clone(),
                                    0,
                                    ChatDelta::default(),
                                    finish_reason,
                                ));
                            }
                        }
                        "message_stop" => {
                            let completion_tokens = completion_tokens
                                .unwrap_or_else(|| estimate_tokens_for_text(&completion_text));
                            yield ProviderStreamEvent::Completed {
                                usage: TokenUsage {
                                    prompt_tokens,
                                    completion_tokens,
                                    total_tokens: prompt_tokens + completion_tokens,
                                },
                            };
                            return;
                        }
                        _ => {}
                    }
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
struct AnthropicMessagesRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<AnthropicToolChoice>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: serde_json::Value,
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

#[derive(Debug, Deserialize)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    message: Option<AnthropicStreamMessage>,
    #[serde(default)]
    delta: Option<AnthropicStreamDelta>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    content_block: Option<AnthropicContentBlock>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamMessage {
    #[allow(dead_code)]
    model: String,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamDelta {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
}

fn split_system_messages(messages: Vec<ChatMessage>) -> (Option<String>, Vec<AnthropicMessage>) {
    let mut system_messages = Vec::new();
    let mut converted = Vec::new();

    for message in messages {
        match message.role {
            ChatRole::Developer | ChatRole::System => system_messages.push(message.content),
            ChatRole::User => converted.push(AnthropicMessage {
                role: "user",
                content: serde_json::Value::String(message.content),
            }),
            ChatRole::Assistant => converted.push(AnthropicMessage {
                role: "assistant",
                content: serde_json::Value::String(message.content),
            }),
            ChatRole::Tool => converted.push(AnthropicMessage {
                role: "user",
                content: serde_json::json!([{
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.unwrap_or_default(),
                    "content": message.content,
                }]),
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
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: ChatRole::User,
                content: "Question".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: ChatRole::System,
                content: "Second".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
        ]);

        assert_eq!(system.as_deref(), Some("First\n\nSecond"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }
}
