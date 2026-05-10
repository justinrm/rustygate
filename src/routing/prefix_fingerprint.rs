//! Privacy-preserving prefix fingerprinting for inference-aware routing.
//!
//! This module builds a short-lived canonical representation of reusable prompt
//! prefix material, hashes it, and returns only aggregate-safe metadata.

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    models::chat::{ChatCompletionRequest, ChatMessage, ChatRole},
    telemetry::token_estimator::estimate_tokens_for_text,
};

const FINGERPRINT_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixFingerprintConfidence {
    High,
    Low,
}

impl PrefixFingerprintConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixFingerprintResult {
    pub fingerprint: Option<String>,
    pub confidence: PrefixFingerprintConfidence,
    pub prefix_char_length: usize,
    pub prefix_token_estimate: u32,
}

impl PrefixFingerprintResult {
    pub fn is_high_confidence(&self) -> bool {
        self.confidence == PrefixFingerprintConfidence::High && self.fingerprint.is_some()
    }
}

pub fn fingerprint_request(request: &ChatCompletionRequest) -> PrefixFingerprintResult {
    let prefix_messages = stable_prefix_messages(&request.messages);
    let normalized_messages = prefix_messages
        .iter()
        .filter_map(normalize_message)
        .collect::<Vec<_>>();
    let normalized_prefix_text = normalized_messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let prefix_char_length = normalized_prefix_text.chars().count();
    let prefix_token_estimate = estimate_tokens_for_text(&normalized_prefix_text);
    let has_high_value_prefix = normalized_messages.iter().any(|message| {
        matches!(
            message.role,
            ChatRole::System | ChatRole::Developer | ChatRole::Tool
        )
    }) || request
        .tools
        .as_ref()
        .is_some_and(|tools| !tools.is_empty())
        || request.tool_choice.is_some()
        || request.response_format.is_some();

    if !has_high_value_prefix {
        return PrefixFingerprintResult {
            fingerprint: None,
            confidence: PrefixFingerprintConfidence::Low,
            prefix_char_length,
            prefix_token_estimate,
        };
    }

    let canonical = CanonicalPrefix {
        version: FINGERPRINT_VERSION,
        model: request.model.as_deref().unwrap_or_default(),
        messages: normalized_messages,
        tools: request.tools.as_ref(),
        tool_choice: request.tool_choice.as_ref(),
        parallel_tool_calls: request.parallel_tool_calls,
        response_format: request.response_format.as_ref(),
    };
    let fingerprint = serde_json::to_vec(&canonical)
        .ok()
        .map(|encoded| hex_digest(&encoded));

    PrefixFingerprintResult {
        fingerprint,
        confidence: PrefixFingerprintConfidence::High,
        prefix_char_length,
        prefix_token_estimate,
    }
}

fn stable_prefix_messages(messages: &[ChatMessage]) -> &[ChatMessage] {
    let Some(last_user_index) = messages
        .iter()
        .rposition(|message| message.role == ChatRole::User)
    else {
        return messages;
    };

    &messages[..last_user_index]
}

fn normalize_message(message: &ChatMessage) -> Option<CanonicalPrefixMessage<'_>> {
    let content = normalize_text(&message.content);
    if content.is_empty() {
        return None;
    }

    Some(CanonicalPrefixMessage {
        role: message.role.clone(),
        content,
        tool_calls: message.tool_calls.as_ref(),
        tool_call_id: message.tool_call_id.as_deref(),
    })
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

#[derive(Serialize)]
struct CanonicalPrefix<'a> {
    version: u8,
    model: &'a str,
    messages: Vec<CanonicalPrefixMessage<'a>>,
    tools: Option<&'a Vec<crate::models::chat::Tool>>,
    tool_choice: Option<&'a crate::models::chat::ToolChoice>,
    parallel_tool_calls: Option<bool>,
    response_format: Option<&'a serde_json::Value>,
}

#[derive(Serialize)]
struct CanonicalPrefixMessage<'a> {
    role: ChatRole,
    content: String,
    tool_calls: Option<&'a Vec<crate::models::chat::ToolCall>>,
    tool_call_id: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::models::{
        chat::{
            ChatCompletionRequest, ChatMessage, ChatRole, Tool, ToolChoice, ToolChoiceMode,
            ToolFunction,
        },
        responses::ResponseRequest,
    };

    #[test]
    fn same_stable_prefix_with_different_final_user_text_has_same_fingerprint() {
        let first = fingerprint_request(&request_with_final_user_text("summarize account A"));
        let second = fingerprint_request(&request_with_final_user_text("summarize account B"));

        assert!(first.is_high_confidence());
        assert_eq!(first.fingerprint, second.fingerprint);
    }

    #[test]
    fn different_models_produce_different_fingerprints() {
        let mut first = request_with_final_user_text("hello");
        let mut second = first.clone();
        first.model = Some("mock-fast-v1".into());
        second.model = Some("mock-smart-v1".into());

        assert_ne!(
            fingerprint_request(&first).fingerprint,
            fingerprint_request(&second).fingerprint
        );
    }

    #[test]
    fn tool_schema_and_choice_affect_fingerprint() {
        let mut first = request_with_tools("lookup_account");
        first.tool_choice = Some(ToolChoice::Mode(ToolChoiceMode::Auto));
        let mut second = request_with_tools("lookup_invoice");
        second.tool_choice = Some(ToolChoice::Mode(ToolChoiceMode::Required));

        assert_ne!(
            fingerprint_request(&first).fingerprint,
            fingerprint_request(&second).fingerprint
        );
    }

    #[test]
    fn response_format_affects_fingerprint() {
        let mut first = request_with_final_user_text("summarize");
        let mut second = first.clone();
        first.response_format = Some(json!({"type": "json_object"}));
        second.response_format = Some(json!({"type": "text"}));

        assert_ne!(
            fingerprint_request(&first).fingerprint,
            fingerprint_request(&second).fingerprint
        );
    }

    #[test]
    fn user_only_request_is_low_confidence_and_skipped() {
        let request = ChatCompletionRequest {
            model: Some("mock-fast-v1".into()),
            messages: vec![message(ChatRole::User, "dynamic user request")],
            temperature: None,
            max_tokens: None,
            stream: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
        };
        let result = fingerprint_request(&request);

        assert_eq!(result.confidence, PrefixFingerprintConfidence::Low);
        assert_eq!(result.fingerprint, None);
    }

    #[test]
    fn whitespace_normalization_is_deterministic() {
        let mut first = request_with_final_user_text("what next?");
        let mut second = request_with_final_user_text("what next?");
        first.messages[0].content = "You are   a\nhelpful\tassistant.".into();
        second.messages[0].content = " You are a helpful assistant. ".into();

        assert_eq!(
            fingerprint_request(&first).fingerprint,
            fingerprint_request(&second).fingerprint
        );
    }

    #[test]
    fn prefix_length_metadata_uses_normalized_prefix_text() {
        let request = request_with_final_user_text("volatile suffix");
        let result = fingerprint_request(&request);

        assert_eq!(
            result.prefix_char_length,
            "You are a helpful assistant.".len()
        );
        assert_eq!(result.prefix_token_estimate, 5);
    }

    #[test]
    fn responses_after_conversion_can_be_fingerprinted() {
        let request: ResponseRequest = serde_json::from_value(json!({
            "model": "mock-fast-v1",
            "instructions": "You are a helpful assistant.",
            "input": "summarize account A"
        }))
        .unwrap();
        let chat_request = request.into_chat_request();
        let result = fingerprint_request(&chat_request);

        assert!(result.is_high_confidence());
        assert_eq!(result.prefix_token_estimate, 5);
    }

    fn request_with_final_user_text(final_user_text: &str) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: Some("mock-fast-v1".into()),
            messages: vec![
                message(ChatRole::System, "You are a helpful assistant."),
                message(ChatRole::User, final_user_text),
            ],
            temperature: None,
            max_tokens: None,
            stream: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
        }
    }

    fn request_with_tools(name: &str) -> ChatCompletionRequest {
        let mut request = request_with_final_user_text("use the tool");
        request.tools = Some(vec![Tool {
            kind: "function".into(),
            function: ToolFunction {
                name: name.into(),
                description: Some("Lookup data".into()),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"}
                    }
                }),
                strict: Some(true),
            },
        }]);
        request
    }

    fn message(role: ChatRole, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }
}
