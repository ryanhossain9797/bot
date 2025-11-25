use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UserChannel {
    Telegram,
    Discord,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentConversation {
    pub history: Vec<HistoryEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UserState {
    Idle {
        recent_conversation: Option<(RecentConversation, DateTime<Utc>)>,
    },
    AwaitingLLMDecision {
        is_timeout: bool,
        recent_conversation: RecentConversation,
        current_input: LLMInput,
    },
    SendingMessage {
        is_timeout: bool,
        outcome: LLMDecisionType,
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
    pub state: UserState,
    pub last_transition: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolCall {
    GetWeather { location: String },
}

/// Represents the input to the LLM decision-making process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMInput {
    /// A message from the user
    UserMessage(String),
    /// Continuation after a tool execution with the tool result
    ToolResult(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMDecisionType {
    IntermediateToolCall {
        maybe_intermediate_response: Option<String>,
        tool_call: ToolCall,
    },
    Final {
        response: String,
    },
}

/// Represents a single entry in the conversation history
/// History alternates between inputs (LLMInput) and outputs (LLMDecisionType)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HistoryEntry {
    /// An input to the LLM (user message or tool result)
    Input(LLMInput),
    /// An output from the LLM (decision/response)
    Output(LLMDecisionType),
}

#[derive(Clone, Debug, Serialize)]
pub enum UserAction {
    ForceReset,
    NewMessage {
        msg: String,
        start_conversation: bool,
    },
    Timeout,
    LLMDecisionResult(Result<LLMDecisionType, String>),
    MessageSent(Result<(), String>),
    ToolResult(Result<String, String>),
}
