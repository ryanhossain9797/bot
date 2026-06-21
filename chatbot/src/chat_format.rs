//! Model-facing chat "wire" shapes — the OpenAI-ish structures a chat template renders from and
//! serializes into the prompt. Pure data with no logic: the conversions that *build* these live
//! with the domain types (`ConversationMessage` / `LLMResponse`) and the tool definitions
//! (`ToolKind`); the render machinery that *consumes* them lives in `roles::render`.

use serde::Serialize;
use serde_json::{Map, Value};

/// A chat message in the shape the chat template consumes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<MessageToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::bare(ChatRole::System, content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::bare(ChatRole::User, content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::bare(ChatRole::Assistant, content)
    }
    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<MessageToolCall>) -> Self {
        Self { role: ChatRole::Assistant, content: content.into(), tool_calls, tool_call_id: None }
    }
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
    fn bare(role: ChatRole, content: impl Into<String>) -> Self {
        Self { role, content: content.into(), tool_calls: Vec::new(), tool_call_id: None }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: MessageToolCallFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageToolCallFunction {
    pub name: &'static str,
    /// Argument values, already stringified and order-preserving — the template splices each
    /// directly, so no JSON re-parsing happens at render time.
    pub arguments: Map<String, Value>,
}

/// A tool definition in the shape the chat template serializes into the `<tools>` block. The
/// `parameters` JSON schema is genuinely dynamic, so it stays a `Value`; the rest is typed.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: ToolDefFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefFunction {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}
