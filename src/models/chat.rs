use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
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

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub provider: String,
    pub choices: Vec<ChatChoice>,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionChunkResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub provider: String,
    pub choices: Vec<ChatChunkChoice>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatChunkChoice {
    pub index: u32,
    pub delta: ChatDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ChatDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<ChatRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
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

        Ok(())
    }
}

impl ChatCompletionResponse {
    pub fn placeholder(request_id: Uuid, model: String, provider: String) -> Self {
        Self {
            id: openai_id("chatcmpl", request_id),
            object: "chat.completion",
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
            object: "chat.completion.chunk",
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
