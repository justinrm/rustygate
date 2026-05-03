use serde::{Deserialize, Serialize};

use crate::models::chat::{ChatMessage, ChatRole, TokenUsage};

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseRequest {
    pub model: Option<String>,
    pub input: ResponseInput,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ResponseInput {
    Text(String),
    Messages(Vec<ResponseInputMessage>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseInputMessage {
    pub role: ResponseInputRole,
    pub content: ResponseInputContent,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseInputRole {
    System,
    Developer,
    User,
    Assistant,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ResponseInputContent {
    Text(String),
    Parts(Vec<ResponseContentPart>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseContentPart {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseObject {
    pub id: String,
    pub object: &'static str,
    pub created_at: i64,
    pub status: &'static str,
    pub model: String,
    pub output: Vec<ResponseOutput>,
    pub usage: ResponseUsage,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseOutput {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub status: &'static str,
    pub role: &'static str,
    pub content: Vec<ResponseOutputContent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseOutputContent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
    pub annotations: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ResponseUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseStreamEvent<T>
where
    T: Serialize,
{
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(flatten)]
    pub payload: T,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseTextDelta {
    pub item_id: String,
    pub output_index: u32,
    pub content_index: u32,
    pub delta: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseCompleted {
    pub response: ResponseObject,
}

impl ResponseRequest {
    pub fn stream_enabled(&self) -> bool {
        self.stream.unwrap_or(false)
    }

    pub fn into_chat_request(self) -> crate::models::chat::ChatCompletionRequest {
        let mut messages = Vec::new();
        if let Some(instructions) = self.instructions {
            if !instructions.trim().is_empty() {
                messages.push(ChatMessage {
                    role: ChatRole::System,
                    content: instructions,
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }

        match self.input {
            ResponseInput::Text(text) => messages.push(ChatMessage {
                role: ChatRole::User,
                content: text,
                tool_calls: None,
                tool_call_id: None,
            }),
            ResponseInput::Messages(input_messages) => {
                messages.extend(input_messages.into_iter().map(|message| ChatMessage {
                    role: match message.role {
                        ResponseInputRole::System | ResponseInputRole::Developer => {
                            ChatRole::System
                        }
                        ResponseInputRole::User => ChatRole::User,
                        ResponseInputRole::Assistant => ChatRole::Assistant,
                    },
                    content: message.content.into_text(),
                    tool_calls: None,
                    tool_call_id: None,
                }));
            }
        }

        crate::models::chat::ChatCompletionRequest {
            model: self.model,
            messages,
            temperature: self.temperature,
            max_tokens: self.max_output_tokens,
            stream: self.stream,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
        }
    }
}

impl ResponseInputContent {
    fn into_text(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Parts(parts) => parts
                .into_iter()
                .filter(|part| part.kind == "input_text" || part.kind == "text")
                .filter_map(|part| part.text)
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

impl From<TokenUsage> for ResponseUsage {
    fn from(usage: TokenUsage) -> Self {
        Self {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}
