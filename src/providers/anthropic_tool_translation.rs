use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::models::chat::{
    ChatDelta, ChatRole, Tool, ToolCall, ToolCallDelta, ToolCallDeltaFunction, ToolCallFunction,
    ToolChoice, ToolChoiceMode,
};

#[derive(Debug, Clone, Serialize)]
pub struct AnthropicTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicToolChoice {
    Auto,
    Any,
    Tool { name: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub input: Option<Value>,
}

#[derive(Debug, Default)]
pub struct AnthropicToolStreamTranslator {
    current_index: Option<u32>,
    current_id: Option<String>,
    current_name: Option<String>,
    current_arguments: String,
}

impl AnthropicToolStreamTranslator {
    pub fn content_block_start(
        &mut self,
        index: u32,
        block: &AnthropicContentBlock,
    ) -> Option<ChatDelta> {
        if block.kind != "tool_use" {
            return None;
        }
        self.current_index = Some(index);
        self.current_id = block.id.clone();
        self.current_name = block.name.clone();
        self.current_arguments.clear();

        Some(ChatDelta {
            role: Some(ChatRole::Assistant),
            content: None,
            tool_calls: Some(vec![ToolCallDelta {
                index,
                id: block.id.clone(),
                kind: Some("function".into()),
                function: Some(ToolCallDeltaFunction {
                    name: block.name.clone(),
                    arguments: Some(String::new()),
                }),
            }]),
        })
    }

    pub fn input_json_delta(&mut self, partial_json: String) -> Option<ChatDelta> {
        let index = self.current_index?;
        self.current_arguments.push_str(&partial_json);
        Some(ChatDelta {
            role: None,
            content: None,
            tool_calls: Some(vec![ToolCallDelta {
                index,
                id: None,
                kind: None,
                function: Some(ToolCallDeltaFunction {
                    name: None,
                    arguments: Some(partial_json),
                }),
            }]),
        })
    }
}

pub fn openai_tools_to_anthropic(tools: &[Tool]) -> Vec<AnthropicTool> {
    tools
        .iter()
        .map(|tool| AnthropicTool {
            name: tool.function.name.clone(),
            description: tool.function.description.clone(),
            input_schema: tool.function.parameters.clone(),
        })
        .collect()
}

pub fn openai_tool_choice_to_anthropic(choice: &ToolChoice) -> Option<AnthropicToolChoice> {
    match choice {
        ToolChoice::Mode(ToolChoiceMode::Auto) => Some(AnthropicToolChoice::Auto),
        ToolChoice::Mode(ToolChoiceMode::Required) => Some(AnthropicToolChoice::Any),
        ToolChoice::Mode(ToolChoiceMode::None) => None,
        ToolChoice::Function { function, .. } => Some(AnthropicToolChoice::Tool {
            name: function.name.clone(),
        }),
    }
}

pub fn anthropic_content_to_openai_message(
    blocks: &[AnthropicContentBlock],
) -> (String, Option<Vec<ToolCall>>) {
    let mut text = String::new();
    let mut tool_calls = Vec::new();

    for block in blocks {
        match block.kind.as_str() {
            "text" => {
                if let Some(block_text) = &block.text {
                    text.push_str(block_text);
                }
            }
            "tool_use" => {
                let Some(id) = &block.id else { continue };
                let Some(name) = &block.name else { continue };
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    kind: "function".into(),
                    function: ToolCallFunction {
                        name: name.clone(),
                        arguments: block.input.clone().unwrap_or_else(|| json!({})).to_string(),
                    },
                });
            }
            _ => {}
        }
    }

    let tool_calls = (!tool_calls.is_empty()).then_some(tool_calls);
    (text, tool_calls)
}
