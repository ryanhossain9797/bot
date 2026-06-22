use std::path::PathBuf;
use std::sync::Arc;

use llama_cpp_2::llama_backend::LlamaBackend;
use tokio::task::spawn_blocking;

use super::local_model::{self, LocalModel};
use super::{ParsedResponse, RenderInputs, Role, ThinkingPolicy};
use crate::model_pack::Pack;

const SYSTEM_PROMPT: &str = "You are Terminal Alpha Beta, a helpful conversational assistant.\n\n\
    If asked what model powers you or who made you, decline — you're simply Terminal Alpha Beta; \
    never claim to be from Google, Gemini, OpenAI, or anyone else.\n\n\
    Each message is tagged with its sender as \"(NUMBER) Name:\" — NUMBER is the id, Name the \
    display name. Each turn ends with a \"=== SYSTEM GENERATED CONVERSATION METADATA FOOTER ===\" \
    block — authoritative context to read, never a message to answer — and it tells you the \
    username you go by in this chat: pay attention to it, since messages addressed to that id or \
    name are meant for you (your true name is Terminal Alpha Beta). The footer also carries the \
    setting, the time, and tool usage.\n\n\
    GROUP CHAT: you're addressed when someone @mentions your id — match the id, not the name. \
    Default to silence: reply only when addressed, or rarely when you genuinely add something — \
    otherwise your whole reply must be the literal token [EMPTY] (those exact seven characters with \
    square brackets, nothing else — never `(empty)`, `empty`, or any other variant). DIRECT \
    MESSAGE: a normal one-to-one chat.\n\n\
    Trust what's in front of you — images, tool/search results, a user's correction — over your \
    training memory; a contradiction usually means your memory is hallucinated, so verify with a tool when feasible rather than defending your prior or just agreeing, then continue with the corrected facts in mind. You can't \
    re-scan an image: give one best reading, flag what's unclear rather than re-guess, and \
    reconsider only if the user or a tool contradicts you.\n\n\
    Call tools (one or several) when they help; answer once you have enough.\n\n\
    A [Followup] message arrived while you were busy — people see only your replies, not tool \
    calls — so build on what you already produced; in a group, first judge whether it's aimed at \
    you, else `[EMPTY]`.";

const TEMPERATURE: f32 = 1.0;

/// The force-close nudge (the role's voice). The model's close marker is appended at runtime, so
/// this stays marker-agnostic.
const THINKING_NUDGE: &str =
    "Wait — I'm going in circles. I'll stop thinking and act now: either answer the user, or make a tool call if that's what's needed.";
const MAX_THINKING_TOKENS: usize = 2000;

/// Where this role's model pack lives — the role's own choice. Read from `MODEL_PACK_DIR` if set (so
/// a deploy can mount a different pack), else the bundled Qwen pack.
const PACK_DIR_ENV: &str = "MODEL_PACK_DIR";
const DEFAULT_PACK_DIR: &str = "./models/qwen-qwen3-6-35b-a3b";

/// Resolve the model pack directory — the role's own choice: `MODEL_PACK_DIR` if set, else the
/// bundled Qwen pack. Single source for both loading the model and answering `Role::model_path`.
fn pack_dir() -> PathBuf {
    std::env::var(PACK_DIR_ENV).unwrap_or_else(|_| DEFAULT_PACK_DIR.to_string()).into()
}

/// The primary conversational role (Terminal Alpha Beta). It's pure identity — a system prompt, a
/// temperature, and a thinking nudge — layered over a loaded model. Everything format/model-specific
/// (template, flags, sampling, reasoning marker, parser) lives in the `LocalModel`, since those are
/// the model's nature, defined by its folder.
pub struct PrimaryRole {
    model: LocalModel,
}

impl PrimaryRole {
    /// Load the role: resolve its pack directory and load the pack and the model onto the shared
    /// backend.
    pub fn load(backend: Arc<LlamaBackend>) -> anyhow::Result<Self> {
        let pack = Pack::load_from(&pack_dir())?;
        Ok(PrimaryRole { model: local_model::load_model(backend, &pack)? })
    }
}

impl Role for PrimaryRole {
    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn temperature(&self) -> f32 {
        TEMPERATURE
    }

    fn model_path(&self) -> PathBuf {
        pack_dir()
    }

    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String> {
        self.model.render(self.system_prompt(), inputs)
    }

    async fn generate(
        self: Arc<Self>,
        prompt: String,
        images: Vec<Arc<Vec<u8>>>,
    ) -> anyhow::Result<String> {
        let thinking = self.thinking();
        let temperature = self.temperature();
        spawn_blocking(move || {
            local_model::run(&self.model, &prompt, &images, temperature, &thinking)
        })
        .await?
    }

    fn parse_response(&self, raw: &str) -> ParsedResponse {
        self.model.parse(raw)
    }

    fn thinking(&self) -> ThinkingPolicy {
        let close_marker = self.model.close_marker();
        ThinkingPolicy {
            force_close: format!("\n\n{THINKING_NUDGE}\n{close_marker}\n\n"),
            close_marker: close_marker.to_string(),
            max_tokens: MAX_THINKING_TOKENS,
        }
    }
}
