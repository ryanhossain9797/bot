mod local_model;
mod parsers;
mod primary_role;
mod render;

use std::path::PathBuf;
use std::sync::Arc;

use crate::chat_format::{ChatMessage, ToolDefinition};

pub use primary_role::PrimaryRole;

pub trait Role: Send + Sync {
    fn system_prompt(&self) -> &str;
    fn temperature(&self) -> f32;
    #[allow(dead_code)]
    fn model_path(&self) -> PathBuf;
    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String>;
    async fn generate(
        self: Arc<Self>,
        prompt: String,
        images: Vec<Arc<Vec<u8>>>,
    ) -> anyhow::Result<String>;
    fn parse_response(&self, raw: &str) -> ParsedResponse;
    fn thinking(&self) -> ThinkingPolicy;
}

pub struct RenderInputs<'a> {
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
