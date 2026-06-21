use std::path::{Path, PathBuf};

use super::{render, FormatFlags, ParsedResponse, RenderInputs, Role, ThinkingPolicy};

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

/// The primary conversational role (Terminal Alpha Beta). Holds the pack's template (loaded once),
/// the pack directory, the model's render flags, and the model's reasoning close marker; the prompt
/// and temperature are fixed.
pub struct PrimaryRole {
    template: String,
    #[allow(dead_code)]
    model_dir: PathBuf,
    flags: FormatFlags,
    close_marker: String,
}

impl PrimaryRole {
    pub fn new(template: String, model_dir: PathBuf, flags: FormatFlags, close_marker: String) -> Self {
        PrimaryRole { template, model_dir, flags, close_marker }
    }
}

impl Role for PrimaryRole {
    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn temperature(&self) -> f32 {
        TEMPERATURE
    }

    fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String> {
        render(&self.template, self.system_prompt(), inputs, self.flags)
    }

    fn parse_response(&self, raw: &str) -> ParsedResponse {
        super::parse(raw)
    }

    fn thinking(&self) -> ThinkingPolicy {
        ThinkingPolicy {
            force_close: format!("\n\n{THINKING_NUDGE}\n{}\n\n", self.close_marker),
            close_marker: self.close_marker.clone(),
            max_tokens: MAX_THINKING_TOKENS,
        }
    }
}
