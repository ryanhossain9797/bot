mod parse;
mod primary_role;
mod render;

use std::path::Path;

use crate::chat_format::{ChatMessage, ToolDefinition};

pub use primary_role::PrimaryRole;

/// A role owns the identity/format contract (system prompt, footer placement, sampling temperature)
/// and the path to its model pack, and turns conversation data into a prompt and raw output back
/// into structured pieces. Everything model-specific lives in files under the pack — the role only
/// holds the loaded template + the pack path. The render/parse machinery lives in the `render` and
/// `parse` submodules.
pub trait Role: Send + Sync {
    fn system_prompt(&self) -> &str;
    fn temperature(&self) -> f32;
    /// Path to this role's model pack. Part of the Role contract; reserved for diagnostics and
    /// multi-model selection.
    #[allow(dead_code)]
    fn model_dir(&self) -> &Path;
    /// Render the final prompt string from conversation inputs (option-b: JSON rendered in Rust).
    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String>;
    /// Parse raw generation into reasoning / content / tool calls.
    fn parse_response(&self, raw: &str) -> ParsedResponse;
    /// How an over-long reasoning block is force-closed during generation.
    fn thinking(&self) -> ThinkingPolicy;
}

pub struct RenderInputs<'a> {
    /// Conversation messages (history + live), without the system prompt or footer.
    pub messages: &'a [ChatMessage],
    /// Tool definitions, or `None` when tools are disabled this turn.
    pub tools: Option<&'a [ToolDefinition]>,
    /// Dynamic metadata footer; the role places it (a final system block before the gen prompt).
    pub footer: Option<&'a str>,
}

/// Model-format render flags, sourced from the pack manifest `[format]` section.
#[derive(Clone, Copy)]
pub struct FormatFlags {
    pub enable_thinking: bool,
    pub add_generation_prompt: bool,
}

pub struct ParsedToolCall {
    pub name: String,
    /// Arguments as a JSON object string, ready for `ToolType::bind`.
    pub arguments: String,
}

pub struct ParsedResponse {
    pub reasoning: String,
    pub content: String,
    pub tool_calls: Vec<ParsedToolCall>,
}

/// The reasoning-block policy the generation loop enforces. Composed by the role from its own
/// nudge prose and the model's `close_marker` (sourced from the pack manifest).
#[derive(Clone)]
pub struct ThinkingPolicy {
    /// Text injected to force-close reasoning once the budget is hit (ends with `close_marker`).
    pub force_close: String,
    /// The model's reasoning close marker, e.g. `</think>`.
    pub close_marker: String,
    /// Token budget after which `force_close` is injected.
    pub max_tokens: usize,
}
