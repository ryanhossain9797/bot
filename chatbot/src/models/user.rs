use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MAX_SEARCH_DESCRIPTION_LENGTH: usize = 200;
pub const MAX_TOOL_OUTPUT_LENGTH: usize = 800;
pub const MAX_HISTORY_TEXT_LENGTH: usize = 50;

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

#[derive(Debug, Clone, Serialize, Deserialize, ollama_rs::generation::parameters::JsonSchema)]
pub enum MathOperation {
    Add(f32, f32),
    Sub(f32, f32),
    Mul(f32, f32),
    Div(f32, f32),
    Exp(f32, f32),
}

#[derive(Debug, Clone, Serialize, Deserialize, ollama_rs::generation::parameters::JsonSchema)]
pub enum ToolCall {
    RecallHistory,
    GetWeather { location: String },
    WebSearch { query: String },
    MathCalculation { operations: Vec<MathOperation> },
    VisitUrl { url: String },
}

/// Represents the input to the LLM decision-making process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMInput {
    /// A message from the user
    UserMessage(String),
    /// Continuation after a tool execution with the tool result
    ToolResult(String),
}

impl LLMInput {
    pub fn format(&self, truncate: bool) -> String {
        match self {
            LLMInput::UserMessage(msg) => {
                let mut content = msg.clone();
                if truncate && content.len() > MAX_HISTORY_TEXT_LENGTH {
                    content.truncate(MAX_HISTORY_TEXT_LENGTH);
                    content.push_str("... (truncated)");
                }
                format!("<|im_start|>user\n{}<|im_end|>", content)
            }
            LLMInput::ToolResult(result) => {
                let mut content = result.clone();
                if truncate && content.len() > MAX_HISTORY_TEXT_LENGTH {
                    content.truncate(MAX_HISTORY_TEXT_LENGTH);
                    content.push_str("... (truncated)");
                }
                format!("<|im_start|>user\n[TOOL RESULT]:\n{}<|im_end|>", content)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ollama_rs::generation::parameters::JsonSchema)]
pub enum LLMDecisionType {
    IntermediateToolCall {
        thoughts: String,
        /// A brief message to the user notifying them of the current progress (e.g., "Searching for...")
        progress_notification: Option<String>,
        tool_call: ToolCall,
    },
    Final {
        response: String,
    },
}

impl LLMDecisionType {
    pub fn format_output(&self) -> String {
        match self {
            LLMDecisionType::Final { response } => {
                let mut content = response.clone();
                if content.len() > MAX_HISTORY_TEXT_LENGTH {
                    content.truncate(MAX_HISTORY_TEXT_LENGTH);
                    content.push_str("... (truncated)");
                }
                format!("<|im_start|>assistant\n{}<|im_end|>", content)
            }
            LLMDecisionType::IntermediateToolCall {
                thoughts,
                progress_notification,
                tool_call,
            } => {
                let mut lines = Vec::new();
                let mut thoughts_content = thoughts.clone();
                if thoughts_content.len() > MAX_HISTORY_TEXT_LENGTH {
                    thoughts_content.truncate(MAX_HISTORY_TEXT_LENGTH);
                    thoughts_content.push_str("... (truncated)");
                }
                lines.push(format!("THOUGHTS: {}", thoughts_content));
                if let Some(msg) = progress_notification {
                    lines.push(format!("INTERMEDIATE PROGRESS: {}", msg));
                }
                lines.push(format!("CALL TOOL: {:?}", tool_call));
                format!("<|im_start|>assistant\n{}<|im_end|>", lines.join("\n"))
            }
        }
    }
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

impl HistoryEntry {
    pub fn format(&self, truncate: bool) -> String {
        match self {
            HistoryEntry::Input(input) => input.format(truncate),
            HistoryEntry::Output(output) => {
                if truncate {
                    output.format_output()
                } else {
                    match output {
                        LLMDecisionType::Final { response } => {
                            format!("<|im_start|>assistant\n{}<|im_end|>", response)
                        }
                        LLMDecisionType::IntermediateToolCall {
                            thoughts,
                            progress_notification,
                            tool_call,
                        } => {
                            let mut lines = Vec::new();
                            lines.push(format!("THOUGHTS: {}", thoughts));
                            if let Some(msg) = progress_notification {
                                lines.push(format!("INTERMEDIATE PROGRESS: {}", msg));
                            }
                            lines.push(format!("CALL TOOL: {:?}", tool_call));
                            format!("<|im_start|>assistant\n{}<|im_end|>", lines.join("\n"))
                        }
                    }
                }
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
    LLMDecisionResult(Result<LLMDecisionType, String>),
    MessageSent(Result<(), String>),
    ToolResult(Result<String, String>),
}

impl std::fmt::Debug for UserAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForceReset => write!(f, "ForceReset"),
            Self::NewMessage {
                msg,
                start_conversation,
            } => f
                .debug_struct("NewMessage")
                .field("msg", msg)
                .field("start_conversation", start_conversation)
                .finish(),
            Self::Timeout => write!(f, "Timeout"),
            Self::LLMDecisionResult(res) => f.debug_tuple("LLMDecisionResult").field(res).finish(),
            Self::MessageSent(res) => f.debug_tuple("MessageSent").field(res).finish(),
            Self::ToolResult(res) => match res {
                Ok(content) => {
                    let mut s = content.clone();
                    if s.len() > MAX_TOOL_OUTPUT_LENGTH {
                        s.truncate(MAX_TOOL_OUTPUT_LENGTH);
                        s.push_str("... (truncated)");
                    }
                    f.debug_tuple("ToolResult")
                        .field(&Ok::<String, String>(s))
                        .finish()
                }
                Err(e) => f
                    .debug_tuple("ToolResult")
                    .field(&Err::<String, String>(e.clone()))
                    .finish(),
            },
        }
    }
}
