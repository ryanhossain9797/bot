use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::types::media::MessageImage;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;
use strum::{EnumDiscriminants, EnumIter};

pub const MAX_SEARCH_DESCRIPTION_LENGTH: usize = 2000;

pub const MAX_TOOL_ROUNDS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Platform {
    Telegram,
    Discord,
}
impl Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Platform::Telegram => write!(f, "Telegram"),
            Platform::Discord => write!(f, "Discord"),
        }
    }
}

impl Platform {
    fn to_string(&self) -> &'static str {
        match self {
            Platform::Telegram => "Telegram",
            Platform::Discord => "Discord",
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub struct ConversationId(pub Platform, pub String);

impl Display for ConversationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}_{}", self.0.to_string(), self.1)
    }
}

impl re_framework::EntityId for ConversationId {
    fn get_id_string(&self) -> String {
        self.to_string()
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
pub enum ConversationState {
    Idle {
        recent_conversation: Option<(RecentConversation, DateTime<Utc>)>,
    },
    CommitingToMemory {
        recent_conversation: RecentConversation,
    },
    AwaitingLLMDecision {
        history: Vec<HistoryEntry>,
        current_input: LLMInput,
                tool_rounds: usize,
    },
    SendingMessage {
        outcome: LLMResponse,
        recent_conversation: RecentConversation,
        tool_rounds: usize,
    },
    RunningTools {
        recent_conversation: RecentConversation,
        tool_rounds: usize,
                pending_tools: HashMap<String, ToolCall>,
                completed_tools: Vec<(ToolCall, ToolResultData)>,
    },
}
impl Default for ConversationState {
    fn default() -> Self {
        ConversationState::Idle {
            recent_conversation: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub text: String,
    pub queued: bool,
        pub user_id: String,
        pub name: String,
        pub images: Vec<MessageImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationConstructor {
        pub id: ConversationId,
        pub is_group: bool,
        pub bot_identity: String,
}

impl re_framework::Identified for ConversationConstructor {
    type Id = ConversationId;
    fn get_id(&self) -> &ConversationId {
        &self.id
    }
}

impl ConversationMessage {
        pub fn to_content(&self) -> String {
        if self.queued {
            format!("[Followup] {}", self.text)
        } else {
            self.text.clone()
        }
    }

    pub fn redacted(&self) -> ConversationMessage {
        ConversationMessage {
            images: self.images.iter().map(MessageImage::dehydrated).collect(),
            ..self.clone()
        }
    }

    /// Content for the model plus the ordered bytes of any hydrated images. Each hydrated
    /// image contributes one `marker` line (the LLM layer splices its bitmap there) and its
    /// bytes; dehydrated images contribute a note explaining they were seen earlier.
    pub fn content_and_media(&self, marker: &str) -> (String, Vec<Arc<Vec<u8>>>) {
        let mut parts: Vec<String> = Vec::new();
        let base = self.to_content();
        if !base.is_empty() {
            parts.push(base);
        }

        let mut bytes = Vec::new();
        let mut dehydrated = 0usize;
        for image in &self.images {
            match image.hydrated_bytes() {
                Some(b) => {
                    parts.push(marker.to_string());
                    bytes.push(b);
                }
                None => dehydrated += 1,
            }
        }
        if dehydrated > 0 {
            parts.push(format!(
                "[{dehydrated} image{} from earlier in the conversation — you saw {} at the time, not re-attached here]",
                if dehydrated == 1 { "" } else { "s" },
                if dehydrated == 1 { "it" } else { "them" }
            ));
        }

        (parts.join("\n"), bytes)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub pending: Vec<ConversationMessage>,
    pub state: ConversationState,
    pub last_transition: DateTime<Utc>,
        pub is_group: bool,
        pub bot_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMInput {
    ConversationMessage(ConversationMessage),
                ToolResults(Vec<ToolResult>, Option<ConversationMessage>),
}

impl LLMInput {
    pub fn redacted(&self) -> LLMInput {
        match self {
            LLMInput::ConversationMessage(m) => LLMInput::ConversationMessage(m.redacted()),
            LLMInput::ToolResults(results, user) => {
                LLMInput::ToolResults(results.clone(), user.as_ref().map(ConversationMessage::redacted))
            }
        }
    }

    /// OpenAI-shaped messages plus the ordered bytes of any hydrated images they carry.
    /// `marker` is the media marker spliced in for each hydrated image.
    pub fn messages_and_media(&self, marker: &str) -> (Vec<Value>, Vec<Arc<Vec<u8>>>) {
        match self {
            LLMInput::ConversationMessage(msg) => {
                let (content, bytes) = msg.content_and_media(marker);
                (vec![json!({ "role": "user", "content": content })], bytes)
            }
            LLMInput::ToolResults(results, user_msg) => {
                let mut messages: Vec<Value> = results
                    .iter()
                    .map(|r| json!({ "role": "tool", "tool_call_id": r.id, "content": r.data.actual }))
                    .collect();
                let mut bytes = Vec::new();
                if let Some(msg) = user_msg {
                    let (content, b) = msg.content_and_media(marker);
                    messages.push(json!({ "role": "user", "content": content }));
                    bytes = b;
                }
                (messages, bytes)
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool_type: ToolType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub data: ToolResultData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
        pub thoughts: String,
        pub message: Option<String>,
        pub tool_calls: Vec<ToolCall>,
}

impl LLMResponse {
        pub fn is_empty(&self) -> bool {
        self.message.as_deref().map_or(true, str::is_empty) && self.tool_calls.is_empty()
    }

    pub fn to_openai_message(&self) -> Value {
        let content = if self.is_empty() {
            "(stayed silent — chose not to reply)"
        } else {
            self.message.as_deref().unwrap_or("")
        };
        let mut msg = json!({
            "role": "assistant",
            "content": content,
        });
        if !self.tool_calls.is_empty() {
            msg["tool_calls"] = Value::Array(
                self.tool_calls
                    .iter()
                    .map(|tc| json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.tool_type.wire_name(),
                            "arguments": tc.tool_type.wire_arguments()
                        }
                    }))
                    .collect(),
            );
        }
        msg
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
                LLMInput::ConversationMessage(m) => format!("User:\n{}", m.text),
                LLMInput::ToolResults(results, user_msg) => {
                    let joined = results
                        .iter()
                        .map(|r| r.data.simplified.clone())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let assistant = format!("Assistant:\n{joined}");
                    match user_msg {
                        Some(msg) => format!("{assistant}\nUser:\n{}", msg.text),
                        None => assistant,
                    }
                }
            },
            HistoryEntry::Output(LLMResponse { message, tool_calls, .. }) => {
                let mut parts = Vec::new();
                if let Some(msg) = message {
                    parts.push(msg.clone());
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("{tool_calls:?}"));
                }
                format!("Assistant:\n{}", parts.join("\n"))
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ConversationAction {
    ForceReset,
    NewMessage {
                msg: String,
        user_id: String,
        name: String,
        images: Vec<MessageImage>,
    },
    Timeout,
    CommitResult(Result<(), String>),
    LLMDecisionResult(Result<LLMResponse, String>),
    MessageSent(Result<(), String>),
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

impl std::fmt::Debug for ConversationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversationAction::ForceReset => write!(f, "ForceReset"),
            ConversationAction::NewMessage { .. } => write!(f, "NewMessage"),
            ConversationAction::Timeout => write!(f, "Timeout"),
            ConversationAction::CommitResult(_) => write!(f, "CommitResult"),
            ConversationAction::LLMDecisionResult(_) => write!(f, "LLMDecisionResult"),
            ConversationAction::MessageSent(_) => write!(f, "MessageSent"),
            ConversationAction::ToolResult { .. } => write!(f, "ToolResult"),
        }
    }
}
