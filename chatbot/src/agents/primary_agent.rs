use super::Agent;

// Minimal, focused persona. Tool/format instructions are NOT hand-written here — the model's
// native chat template owns those (see model_reference/qwen.md). The current date/time is plugged
// in at render time (see `Agent::system_content`).
const SYSTEM_PROMPT: &'static str = "You are Terminal Alpha Beta, a helpful conversational assistant.";

// Qwen3 recommended sampling temperature (LM Studio preset: 0.6).
const TEMPERATURE: f32 = 0.6;

pub const PRIMARY_AGENT_IMPL: Agent = Agent::new(SYSTEM_PROMPT, TEMPERATURE);
