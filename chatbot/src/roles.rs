mod local_model;
mod parsers;
mod primary_role;
mod render;
mod utility_role;

use std::path::PathBuf;
use std::sync::Arc;

use crate::chat_format::{ChatMessage, ToolDefinition};

pub use primary_role::PrimaryRole;
pub use utility_role::UtilityRole;

pub trait Role: Send + Sync {
    fn model_path(&self) -> PathBuf;
    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String>;
    async fn generate(
        self: Arc<Self>,
        prompt: String,
        images: Vec<Arc<Vec<u8>>>,
        temperature: f32,
    ) -> anyhow::Result<String>;
    fn parse_response(&self, raw: &str) -> ParsedResponse;
    fn thinking(&self) -> ThinkingPolicy;
}

pub struct RenderInputs<'a> {
    pub system: Option<&'a str>,
    pub messages: &'a [ChatMessage],
    pub tools: Option<&'a [ToolDefinition]>,
    pub footer: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub struct FormatFlags {
    pub enable_thinking: bool,
    pub add_generation_prompt: bool,
}

pub struct ParsedToolCall {
    pub name: String,
    pub arguments: String,
}

pub struct ParsedResponse {
    pub reasoning: String,
    pub content: String,
    pub tool_calls: Vec<ParsedToolCall>,
}

#[derive(Clone)]
pub struct ThinkingPolicy {
    pub force_close: String,
    pub close_marker: String,
    pub max_tokens: usize,
}
