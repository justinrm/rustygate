use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{compat::openai_id, error::AppError};

#[derive(Debug, Clone)]
pub struct ChatValidationLimits {
    pub max_messages_per_request: usize,
    pub max_message_content_chars: usize,
}

impl Default for ChatValidationLimits {
    fn default() -> Self {
        Self {
            max_messages_per_request: 64,
            max_message_content_chars: 8_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stream: Option<bool>,
    #[serde(default)]
    pub tools: Option<Vec<Tool>>,
    #[serde(default)]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub response_format: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ToolChoice {
    Mode(ToolChoiceMode),
    Function {
        #[serde(rename = "type")]
        kind: String,
        function: ToolChoiceFunction,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoiceMode {
    Auto,
    None,
    Required,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolChoiceFunction {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    Developer,
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub provider: String,
    pub choices: Vec<ChatChoice>,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunkResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub provider: String,
    pub choices: Vec<ChatChunkChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunkChoice {
    pub index: u32,
    pub delta: ChatDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<ChatRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDelta {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<ToolCallDeltaFunction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDeltaFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl ChatCompletionRequest {
    pub fn stream_enabled(&self) -> bool {
        self.stream.unwrap_or(false)
    }

    pub fn validate(
        &self,
        request_id: Option<Uuid>,
        limits: &ChatValidationLimits,
    ) -> Result<(), AppError> {
        if self
            .model
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            return Err(AppError::InvalidRequest {
                message: "model must be provided".into(),
                request_id,
            });
        }

        if self.messages.is_empty() {
            return Err(AppError::InvalidRequest {
                message: "messages must contain at least one item".into(),
                request_id,
            });
        }
        if self.messages.len() > limits.max_messages_per_request {
            return Err(AppError::InvalidRequest {
                message: format!(
                    "messages must contain at most {} items",
                    limits.max_messages_per_request
                ),
                request_id,
            });
        }

        if self
            .messages
            .iter()
            .any(|message| message.content.trim().is_empty())
        {
            return Err(AppError::InvalidRequest {
                message: "message content must not be empty".into(),
                request_id,
            });
        }
        if self
            .messages
            .iter()
            .any(|message| message.content.chars().count() > limits.max_message_content_chars)
        {
            return Err(AppError::InvalidRequest {
                message: format!(
                    "message content must be at most {} characters",
                    limits.max_message_content_chars
                ),
                request_id,
            });
        }

        if let Some(tools) = &self.tools {
            if tools.len() > 128 {
                return Err(AppError::InvalidRequest {
                    message: "tools must contain at most 128 entries".into(),
                    request_id,
                });
            }
            for tool in tools {
                if tool.kind != "function" || tool.function.name.trim().is_empty() {
                    return Err(AppError::InvalidRequest {
                        message: "each tool must be a named function tool".into(),
                        request_id,
                    });
                }
                if serde_json::to_string(&tool.function.parameters)
                    .map(|value| value.len() > 8_192)
                    .unwrap_or(true)
                {
                    return Err(AppError::InvalidRequest {
                        message: "tool parameter schemas must be at most 8192 bytes".into(),
                        request_id,
                    });
                }
            }
        }
        if let Some(ToolChoice::Function { kind, function }) = &self.tool_choice {
            if kind != "function" {
                return Err(AppError::InvalidRequest {
                    message: "tool_choice function selectors must use type `function`".into(),
                    request_id,
                });
            }
            let tool_exists = self
                .tools
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|tool| tool.function.name == function.name);
            if !tool_exists {
                return Err(AppError::InvalidRequest {
                    message: format!("tool_choice references undefined tool `{}`", function.name),
                    request_id,
                });
            }
        }

        Ok(())
    }
}

impl ChatCompletionResponse {
    pub fn placeholder(request_id: Uuid, model: String, provider: String) -> Self {
        Self {
            id: openai_id("chatcmpl", request_id),
            object: "chat.completion".into(),
            created: OffsetDateTime::now_utc().unix_timestamp(),
            model,
            provider,
            choices: Vec::new(),
            usage: TokenUsage::default(),
        }
    }
}

impl ChatCompletionChunkResponse {
    pub fn from_delta(
        id: Uuid,
        created: i64,
        model: String,
        provider: String,
        index: u32,
        delta: ChatDelta,
        finish_reason: Option<String>,
    ) -> Self {
        Self {
            id: openai_id("chatcmpl", id),
            object: "chat.completion.chunk".into(),
            created,
            model,
            provider,
            choices: vec![ChatChunkChoice {
                index,
                delta,
                finish_reason,
            }],
        }
    }
}
