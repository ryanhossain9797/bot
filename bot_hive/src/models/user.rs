use std::{fmt::Display, sync::Arc};

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
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

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct UserId(pub UserChannel, pub String);

#[derive(Clone, Debug)]
pub struct RecentConversation {
    pub summary: String,
}

#[derive(Clone, Debug)]
pub enum UserState {
    Idle(Option<(RecentConversation, DateTime<Utc>)>),
    AwaitingLLMDecision {
        is_timeout: bool,
        previous_tool_calls: Vec<String>,
    },
    SendingMessage {
        is_timeout: bool,
        outcome: MessageOutcome,
        recent_conversation: RecentConversation,
        previous_tool_calls: Vec<String>,
    },
    RunningTool {
        is_timeout: bool,
        recent_conversation: RecentConversation,
        previous_tool_calls: Vec<String>,
    },
}
impl Default for UserState {
    fn default() -> Self {
        UserState::Idle(None)
    }
}

#[derive(Clone, Default)]
pub struct User {
    pub state: UserState,
    pub last_transition: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub enum ToolCall {
    GetWeather { location: String },
}

#[derive(Debug, Clone, Deserialize)]
pub enum MessageOutcome {
    IntermediateToolCall {
        maybe_intermediate_response: Option<String>,
        tool_call: ToolCall,
    },
    Final {
        response: String,
    },
}

#[derive(Clone, Debug)]
pub enum UserAction {
    ForceReset,
    NewMessage {
        msg: String,
        start_conversation: bool,
    },
    Timeout,
    LLMDecisionResult(Arc<anyhow::Result<(String, MessageOutcome)>>),
    MessageSent(Arc<anyhow::Result<()>>),
    ToolResult(Arc<anyhow::Result<String>>),
}
