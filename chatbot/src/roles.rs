mod local_model;
mod parsers;
mod primary_role;
mod render;

use std::sync::Arc;

use crate::chat_format::{ChatMessage, ToolDefinition};

pub use primary_role::PrimaryRole;

/// A role owns everything about producing output from one model: its identity/format contract
/// (system prompt, footer placement, sampling temperature), the loaded model itself, and the logic
/// to turn conversation data into a prompt, run inference, and decode raw output back into structured
/// pieces. Everything model-specific lives in files under the pack — the role loads them once. The
/// render/parse/inference machinery lives in the `render`, `parsers`, and `local_model` submodules.
///
/// The contract is deliberately location-agnostic: `generate` takes only a prompt and produces text,
/// with no mention of a backend or any inference engine. A local role holds its model (and the
/// shared `LlamaBackend`) internally; a future remote role could satisfy the same contract with an
/// HTTP call. Where the model lives is the implementor's business, not the trait's.
pub trait Role: Send + Sync {
    fn system_prompt(&self) -> &str;
    fn temperature(&self) -> f32;
    /// Render the final prompt string from conversation inputs (JSON serialized in Rust, not via
    /// in-template `tojson`).
    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String>;
    /// Run inference on a rendered prompt and return the raw generated text. Takes `Arc<Self>` so it
    /// can move into a blocking inference task; everything else it needs, it already holds.
    async fn generate(
        self: Arc<Self>,
        prompt: String,
        images: Vec<Arc<Vec<u8>>>,
    ) -> anyhow::Result<String>;
    /// Parse raw generation into reasoning / content / tool calls. Like rendering, this is the
    /// implementor's job: the reasoning marker and tool-call grammar are the model's own, so a
    /// different model family brings its own parser rather than reusing this one.
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
