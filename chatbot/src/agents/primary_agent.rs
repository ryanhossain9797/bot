use super::Agent;

const SYSTEM_PROMPT: &'static str = "You are Terminal Alpha Beta, a helpful conversational assistant.";
const TEMPERATURE: f32 = 0.6;

pub const PRIMARY_AGENT_IMPL: Agent = Agent::new(SYSTEM_PROMPT, TEMPERATURE);
