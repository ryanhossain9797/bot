use crate::chat_format::{ChatMessage, MessageToolCall, MessageToolCallFunction};
use crate::types::media::MessageImage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
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

    pub fn formatting_note(&self) -> &'static str {
        match self {
            Platform::Discord => "Platform: Discord — renders basic markdown but NOT tables. For tabular data use an aligned monospace table inside a ```code block```, never bare `| ... |` (it won't align).",
            Platform::Telegram => "Platform: Telegram — supports a limited markdown subset (bold, italic, `code`, links); no tables, headers, or bullet lists. For tabular data use a ```code block```.",
        }
    }

    pub fn subtext(&self, text: &str) -> String {
        match self {
            Platform::Discord => format!("-# *{text}*"),
            Platform::Telegram => format!("_{text}_"),
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

/// Rolling window of the most recent conversation entries — this is the bot's whole memory.
pub const RECENT_WINDOW: usize = 30;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentConversation {
    pub thoughts: String,
    pub history: VecDeque<HistoryEntry>,
}
impl RecentConversation {
    pub fn empty() -> Self {
        RecentConversation {
            thoughts: String::new(),
            history: VecDeque::new(),
        }
    }

    /// Build from a flat history, keeping only the last `RECENT_WINDOW` entries.
    pub fn new(thoughts: String, history: Vec<HistoryEntry>) -> Self {
        let mut window: VecDeque<HistoryEntry> = history.into();
        while window.len() > RECENT_WINDOW {
            window.pop_front();
        }
        RecentConversation {
            thoughts,
            history: window,
        }
    }

    pub fn history(&self) -> Vec<HistoryEntry> {
        self.history.iter().cloned().collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ConversationState {
    Idle {
        recent_conversation: RecentConversation,
    },
    AwaitingLLMDecision {
        history: Vec<HistoryEntry>,
        current_input: LLMInput,
        tool_rounds: usize,
    },
    SendingMessage {
        recent_conversation: RecentConversation,
        post_send: PostSend,
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
            recent_conversation: RecentConversation::empty(),
        }
    }
}

/// What `SendingMessage` does once its in-flight send is confirmed. Each variant
/// carries exactly what its continuation needs — nothing more.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PostSend {
    /// Nothing follows — go Idle.
    Nothing,
    /// The message was delivered; now run these tools.
    CallTools {
        tool_rounds: usize,
        tool_calls: Vec<ToolCall>,
    },
    /// The user-facing part was delivered; now relay the results to the LLM.
    SendToolResponse {
        tool_rounds: usize,
        results: Vec<ToolResult>,
        followup: Option<ConversationMessage>,
    },
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
            LLMInput::ToolResults(results, user) => LLMInput::ToolResults(
                results.clone(),
                user.as_ref().map(ConversationMessage::redacted),
            ),
        }
    }

    pub fn messages_and_media(&self, marker: &str) -> (Vec<ChatMessage>, Vec<Arc<Vec<u8>>>) {
        match self {
            LLMInput::ConversationMessage(msg) => {
                let (content, bytes) = msg.content_and_media(marker);
                (vec![ChatMessage::user(content)], bytes)
            }
            LLMInput::ToolResults(results, user_msg) => {
                let mut messages: Vec<ChatMessage> = Vec::new();
                let mut bytes: Vec<Arc<Vec<u8>>> = Vec::new();
                let mut delivered: Vec<Arc<Vec<u8>>> = Vec::new();

                for r in results {
                    let mut parts: Vec<String> = Vec::new();
                    if !r.data.actual.is_empty() {
                        parts.push(r.data.actual.clone());
                    }
                    if let Some(image) = &r.data.image_for_user {
                        match image.hydrated_bytes() {
                            Some(b) => {
                                parts.push("[The image you produced was delivered to the user as your message — shown below.]".to_string());
                                delivered.push(b);
                            }
                            None => parts.push(
                                "[An image was delivered to the user as your message earlier in the conversation.]".to_string(),
                            ),
                        }
                    }
                    if let Some(image) = &r.data.image_for_assistant {
                        match image.hydrated_bytes() {
                            Some(b) => {
                                parts.push(marker.to_string());
                                bytes.push(b);
                            }
                            None => parts.push(
                                "[A tool-result image from earlier in the conversation — you saw it at the time, not re-attached here.]".to_string(),
                            ),
                        }
                    }
                    messages.push(ChatMessage::tool(r.id.clone(), parts.join("\n")));
                }

                if !delivered.is_empty() {
                    let content = vec![marker.to_string(); delivered.len()].join("\n");
                    messages.push(ChatMessage::assistant(content));
                    bytes.extend(delivered);
                }

                if let Some(msg) = user_msg {
                    let (content, b) = msg.content_and_media(marker);
                    messages.push(ChatMessage::user(content));
                    bytes.extend(b);
                }

                (messages, bytes)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumDiscriminants)]
#[strum_discriminants(name(ToolKind))]
#[strum_discriminants(derive(EnumIter))]
#[strum_discriminants(vis(pub))]
pub enum ToolType {
    WebSearch { query: String },
    VisitUrl { url: String },
    RunBashCommand { command: String },
    ResetBashContainer,
    ViewImage { path: String },
    SendImageToUser { path: String },
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

/// What the model produced for the message slot. `Empty` is a deliberate non-reply
/// (the [EMPTY] token, or a silent tool call); `Malformed` is a failed/blank generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Reply {
    Said(String),
    Empty,
    Malformed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    pub thoughts: String,
    pub reply: Reply,
    pub tool_calls: Vec<ToolCall>,
}

impl LLMResponse {
    pub fn message(&self) -> Option<&str> {
        match &self.reply {
            Reply::Said(text) => Some(text),
            Reply::Empty | Reply::Malformed => None,
        }
    }

    pub fn to_chat_message(&self) -> ChatMessage {
        let content = match &self.reply {
            Reply::Said(text) => text.clone(),
            Reply::Empty if self.tool_calls.is_empty() => "[EMPTY]".to_string(),
            Reply::Empty => String::new(),
            Reply::Malformed => {
                "[MALFORMED — assistant generated no usable output: no message and no tool call]"
                    .to_string()
            }
        };
        if self.tool_calls.is_empty() {
            ChatMessage::assistant(content)
        } else {
            let calls = self
                .tool_calls
                .iter()
                .map(|tc| MessageToolCall {
                    id: tc.id.clone(),
                    kind: "function",
                    function: MessageToolCallFunction {
                        name: tc.tool_type.wire_name(),
                        arguments: tc.tool_type.arguments_map(),
                    },
                })
                .collect();
            ChatMessage::assistant_with_tools(content, calls)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HistoryEntry {
    Input(LLMInput),
    Output(LLMResponse),
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
    /// Image fed back into the model alongside `actual` (multimodal tool result).
    pub image_for_assistant: Option<MessageImage>,
    /// Image delivered straight to the chat, bypassing the model.
    pub image_for_user: Option<MessageImage>,
}

impl ToolResultData {
    /// Text-only result — no images for either side. The common case.
    pub fn text(actual: String, simplified: String) -> Self {
        ToolResultData {
            actual,
            simplified,
            image_for_assistant: None,
            image_for_user: None,
        }
    }
}

impl std::fmt::Debug for ConversationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversationAction::ForceReset => write!(f, "ForceReset"),
            ConversationAction::NewMessage { .. } => write!(f, "NewMessage"),
            ConversationAction::LLMDecisionResult(_) => write!(f, "LLMDecisionResult"),
            ConversationAction::MessageSent(_) => write!(f, "MessageSent"),
            ConversationAction::ToolResult { .. } => write!(f, "ToolResult"),
        }
    }
}
