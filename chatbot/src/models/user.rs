use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::Display;

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
    },
    RunningInternalFunction {
        is_timeout: bool,
        recent_conversation: RecentConversation,
    },
    SendingMessage {
        is_timeout: bool,
        outcome: LLMResponse,
        recent_conversation: RecentConversation,
    },
    RunningTool {
        is_timeout: bool,
        recent_conversation: RecentConversation,
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

/// Represents the input to the LLM decision-making process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMInput {
    /// A message from the user
    UserMessage(String),
    /// Continuation after an internal function execution with the function result
    InternalFunctionResult(InternalFunctionResultData),
    /// Continuation after a tool execution with the tool result
    ToolResult(ToolResultData),
}

impl LLMInput {
    pub fn format(&self) -> String {
        match self {
            LLMInput::UserMessage(msg) => format!("user: {msg}"),
            LLMInput::InternalFunctionResult(InternalFunctionResultData { simplified, .. }) => {
                format!("internal_function_result: {simplified}")
            }
            LLMInput::ToolResult(ToolResultData { simplified, .. }) => {
                format!("tool_result: {simplified}")
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolCall {
    GetWeather { location: String },
    WebSearch { query: String },
    MathCalculation { operations: Vec<MathOperation> },
    VisitUrl { url: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FunctionCall {
    RecallShortTerm { reason: String },
    RecallLongTerm { search_term: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMDecisionType {
    IntermediateToolCall { tool_call: ToolCall },
    InternalFunctionCall { function_call: FunctionCall },
    MessageUser { response: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub thoughts: String,
    pub output: LLMDecisionType,
    pub simple_output: String,
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
                LLMInput::UserMessage(m) => format!("<USER>\n{m}\n\n"),
                LLMInput::InternalFunctionResult(InternalFunctionResultData {
                    simplified, ..
                })
                | LLMInput::ToolResult(ToolResultData { simplified, .. }) => {
                    format!("<SYSTEM>\n{simplified}\n\n")
                }
            },
            HistoryEntry::Output(LLMResponse { simple_output, .. }) => {
                format!("<AGENT>\n{simple_output}\n\n")
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
    InternalFunctionResult(Result<InternalFunctionResultData, String>),
    MessageSent(Result<(), String>),
    ToolResult(Result<ToolResultData, String>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InternalFunctionResultData {
    pub actual: String,
    pub simplified: String,
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
            UserAction::InternalFunctionResult(_) => write!(f, "InternalFunctionResult"),
            UserAction::MessageSent(_) => write!(f, "MessageSent"),
            UserAction::ToolResult(_) => write!(f, "ToolResult"),
        }
    }
}
