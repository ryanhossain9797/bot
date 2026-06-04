use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::Display;
use strum::{EnumDiscriminants, EnumIter};

pub const MAX_SEARCH_DESCRIPTION_LENGTH: usize = 20;

/// Max tool calls the model may make in a single user turn before the loop is cut short. Single
/// source of truth: the state machine enforces it; the system prompt discloses it.
pub const MAX_TOOL_ROUNDS: usize = 10;

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
    RunningTools {
        is_timeout: bool,
        recent_conversation: RecentConversation,
        tool_rounds: usize,
        /// Calls still in flight this batch, keyed by id (id duplicated as the key).
        pending_tools: HashMap<String, ToolCall>,
        /// Calls that have returned, paired with their (error-folded) result.
        completed_tools: Vec<(ToolCall, ToolResultData)>,
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
    /// One turn's batch of tool results (id order), each tagged with the call it answers.
    ToolResults(Vec<ToolResult>),
}

impl LLMInput {
    /// Render to OpenAI messages: a user message is one message; a tool-result batch is one `tool`
    /// message per result (the template groups them into a single tool-response turn).
    pub fn to_openai_messages(&self) -> Vec<Value> {
        match self {
            LLMInput::UserMessage(msg) => vec![json!({ "role": "user", "content": msg })],
            LLMInput::ToolResults(results) => results
                .iter()
                .map(|r| json!({ "role": "tool", "tool_call_id": r.id, "content": r.data.actual }))
                .collect(),
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

/// A tool and its arguments. `ToolKind` (via `EnumDiscriminants`) is the fieldless companion used
/// to build the advertised registry and look tools up by wire name — see `crate::tools`. Pair it
/// with the parser-assigned id via [`ToolCall`] for a concrete invocation.
#[derive(Debug, Clone, Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(name(ToolKind))]
#[strum_discriminants(derive(EnumIter))]
#[strum_discriminants(vis(pub))]
pub enum ToolType {
    GetWeather { location: String },
    MathCalculation { operations: Vec<MathOperation> },
    WebSearch { query: String },
    VisitUrl { url: String },
    RecallShortTerm { reason: String },
    RecallLongTerm { search_term: String },
}

/// A concrete tool invocation: a [`ToolType`] tagged with the id the parser assigned, so a result
/// can be paired back to the call that produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool_type: ToolType,
}

/// A concrete tool result: the data tagged with the id of the call it answers (symmetric to
/// [`ToolCall`]). One per call in a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub data: ToolResultData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMDecisionType {
    IntermediateToolCall { tool_calls: Vec<ToolCall> },
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
            // Tool-call turns carry empty content + a native tool_calls array (one entry per call).
            // name/arguments are derived back from each bound ToolType (the inverse of `bind`).
            LLMDecisionType::IntermediateToolCall { tool_calls } => json!({
                "role": "assistant",
                "content": "",
                "tool_calls": tool_calls
                    .iter()
                    .map(|tc| json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.tool_type.wire_name(),
                            "arguments": tc.tool_type.wire_arguments()
                        }
                    }))
                    .collect::<Vec<_>>()
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
                LLMInput::ToolResults(results) => {
                    let joined = results
                        .iter()
                        .map(|r| r.data.simplified.clone())
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("Assistant:\n{joined}")
                }
            },
            HistoryEntry::Output(LLMResponse { output, .. }) => {
                let text = match output {
                    LLMDecisionType::MessageUser { response } => response.clone(),
                    LLMDecisionType::IntermediateToolCall { tool_calls } => {
                        format!("{tool_calls:?}")
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
    /// One tool's result, tagged with the id of the call it answers (one action per dispatched call).
    ToolResult {
        id: String,
        result: Result<ToolResultData, String>,
    },
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
            UserAction::ToolResult { .. } => write!(f, "ToolResult"),
        }
    }
}
