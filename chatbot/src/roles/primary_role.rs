use std::sync::Arc;

use llama_cpp_2::llama_backend::LlamaBackend;
use tokio::task::spawn_blocking;

use super::engine::{self, GenConfig, PrimaryModel};
use super::{parse, render, FormatFlags, ParsedResponse, RenderInputs, Role, ThinkingPolicy};
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

/// The primary conversational role (Terminal Alpha Beta). Owns its loaded model, the pack's
/// inference config and template, the model's render flags, and its reasoning close marker; the
/// system prompt and temperature are fixed.
pub struct PrimaryRole {
    model: PrimaryModel,
    cfg: GenConfig,
    template: String,
    flags: FormatFlags,
    close_marker: String,
}

impl PrimaryRole {
    /// Load the role from a pack: read its config + template and load the weights into memory,
    /// taking an `Arc` to the shared backend to store inside the model.
    pub fn load(backend: Arc<LlamaBackend>, pack: &Pack) -> anyhow::Result<Self> {
        let model = engine::load_model(backend, pack)?;
        Ok(PrimaryRole {
            model,
            cfg: GenConfig::from_pack(pack),
            template: pack.template.clone(),
            flags: FormatFlags {
                enable_thinking: pack.manifest.format.enable_thinking,
                add_generation_prompt: pack.manifest.format.add_generation_prompt,
            },
            close_marker: pack.manifest.thinking.close_marker.clone(),
        })
    }
}

impl Role for PrimaryRole {
    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn temperature(&self) -> f32 {
        TEMPERATURE
    }

    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String> {
        render::render(&self.template, self.system_prompt(), inputs, self.flags)
    }

    async fn generate(
        self: Arc<Self>,
        prompt: String,
        images: Vec<Arc<Vec<u8>>>,
    ) -> anyhow::Result<String> {
        let thinking = self.thinking();
        let temperature = self.temperature();
        spawn_blocking(move || {
            engine::run(&self.model, &self.cfg, &prompt, &images, temperature, &thinking)
        })
        .await?
    }

    fn parse_response(&self, raw: &str) -> ParsedResponse {
        parse::parse(raw)
    }

    fn thinking(&self) -> ThinkingPolicy {
        ThinkingPolicy {
            force_close: format!("\n\n{THINKING_NUDGE}\n{}\n\n", self.close_marker),
            close_marker: self.close_marker.clone(),
            max_tokens: MAX_THINKING_TOKENS,
        }
    }
}
