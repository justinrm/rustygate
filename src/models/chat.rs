use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::AppError;

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionResponse {
    pub id: Uuid,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub provider: String,
    pub choices: Vec<ChatChoice>,
    pub usage: TokenUsage,
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
    pub fn validate(&self, request_id: Option<Uuid>) -> Result<(), AppError> {
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

        Ok(())
    }
}

impl ChatCompletionResponse {
    pub fn placeholder(request_id: Uuid, model: String, provider: String) -> Self {
        Self {
            id: request_id,
            object: "chat.completion",
            created: OffsetDateTime::now_utc().unix_timestamp(),
            model,
            provider,
            choices: Vec::new(),
            usage: TokenUsage::default(),
        }
    }
}
