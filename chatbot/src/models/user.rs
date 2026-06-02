use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Display;
use strum::{EnumDiscriminants, EnumIter};

pub const MAX_SEARCH_DESCRIPTION_LENGTH: usize = 20;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UserChannel {
    Telegram,
    Discord,
}
impl Display for UserChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserChannel::Telegram => write!(f, "Telegram"),
            UserChannel::Discord => write!(f, "Discord"),
        }
    }
}

impl UserChannel {
    fn to_string(&self) -> &'static str {
        match self {
            UserChannel::Telegram => "Telegram",
            UserChannel::Discord => "Discord",
        }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub struct UserId(pub UserChannel, pub String);

impl Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}_{}", self.0.to_string(), self.1)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentConversation {
    pub thoughts: String,
    pub history: Vec<HistoryEntry>,
}
impl RecentConversation {
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.history.clone()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UserState {
    Idle {
        recent_conversation: Option<(RecentConversation, DateTime<Utc>)>,
    },
    CommitingToMemory {
        recent_conversation: RecentConversation,
    },
    AwaitingLLMDecision {
        is_timeout: bool,
        history: Vec<HistoryEntry>,
        current_input: LLMInput,
        /// Tool calls made so far in this turn (resets to 0 on a new user turn).
        tool_rounds: usize,
    },
    SendingMessage {
        is_timeout: bool,
        outcome: LLMResponse,
        recent_conversation: RecentConversation,
    },
    RunningTool {
        is_timeout: bool,
        recent_conversation: RecentConversation,
        tool_rounds: usize,
    },
}
impl Default for UserState {
    fn default() -> Self {
        UserState::Idle {
            recent_conversation: None,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct User {
    pub pending: Vec<String>,
    pub state: UserState,
    pub last_transition: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMInput {
    UserMessage(String),
    ToolResult(ToolResultData),
}

impl LLMInput {
    pub fn to_openai_message(&self, last_tool_call_id: Option<&str>) -> Value {
        match self {
            LLMInput::UserMessage(msg) => json!({ "role": "user", "content": msg }),
            LLMInput::ToolResult(ToolResultData { actual, .. }) => {
                let id = last_tool_call_id.unwrap_or("call_0");
                json!({ "role": "tool", "tool_call_id": id, "content": actual })
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MathOperation {
    Add(f32, f32),
    Sub(f32, f32),
    Mul(f32, f32),
    Div(f32, f32),
    Exp(f32, f32),
}

/// Every executable tool. `ToolKind` (via `EnumDiscriminants`) is the fieldless companion used to
/// build the advertised registry and look tools up by wire name — see `crate::tools`.
#[derive(Debug, Clone, Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(name(ToolKind))]
#[strum_discriminants(derive(EnumIter))]
#[strum_discriminants(vis(pub))]
pub enum ToolCall {
    GetWeather { location: String },
    MathCalculation { operations: Vec<MathOperation> },
    WebSearch { query: String },
    VisitUrl { url: String },
    RecallShortTerm { reason: String },
    RecallLongTerm { search_term: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMDecisionType {
    IntermediateToolCall { tool_call: ToolCall, id: String },
    MessageUser { response: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    /// Reasoning (`<think>`); kept for logging but never replayed into the next prompt.
    pub thoughts: String,
    pub output: LLMDecisionType,
}

impl LLMResponse {
    pub fn to_openai_message(&self) -> Value {
        match &self.output {
            LLMDecisionType::MessageUser { response } => {
                json!({ "role": "assistant", "content": response })
            }
            // Tool-call turns carry empty content + a native tool_calls array. name/arguments are
            // derived back from the bound ToolCall (the inverse of `ToolCall::bind`).
            LLMDecisionType::IntermediateToolCall { tool_call, id } => json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": tool_call.wire_name(),
                        "arguments": tool_call.wire_arguments()
                    }
                }]
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HistoryEntry {
    Input(LLMInput),
    Output(LLMResponse),
}

impl HistoryEntry {
    pub fn format_simplified(&self) -> String {
        match self {
            HistoryEntry::Input(llm_input) => match llm_input {
                LLMInput::UserMessage(m) => format!("User:\n{m}"),
                LLMInput::ToolResult(ToolResultData { simplified, .. }) => {
                    format!("Assistant:\n{simplified}")
                }
            },
            HistoryEntry::Output(LLMResponse { output, .. }) => {
                let text = match output {
                    LLMDecisionType::MessageUser { response } => response.clone(),
                    LLMDecisionType::IntermediateToolCall { tool_call, .. } => {
                        format!("{tool_call:?}")
                    }
                };
                format!("Assistant:\n{text}")
            }
        }
    }
}

#[derive(Clone, Serialize)]
pub enum UserAction {
    ForceReset,
    NewMessage {
        msg: String,
        start_conversation: bool,
    },
    Timeout,
    CommitResult(Result<(), String>),
    LLMDecisionResult(Result<LLMResponse, String>),
    MessageSent(Result<(), String>),
    ToolResult(Result<ToolResultData, String>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResultData {
    pub actual: String,
    pub simplified: String,
}

impl std::fmt::Debug for UserAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserAction::ForceReset => write!(f, "ForceReset"),
            UserAction::NewMessage { .. } => write!(f, "NewMessage"),
            UserAction::Timeout => write!(f, "Timeout"),
            UserAction::CommitResult(_) => write!(f, "CommitResult"),
            UserAction::LLMDecisionResult(_) => write!(f, "LLMDecisionResult"),
            UserAction::MessageSent(_) => write!(f, "MessageSent"),
            UserAction::ToolResult(_) => write!(f, "ToolResult"),
        }
    }
}
