use crate::chat_format::{ChatMessage, MessageToolCall, MessageToolCallFunction};
use crate::types::media::{Attachment, MessageImage};
use chrono::{DateTime, Duration, Utc};
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
        f.write_str(self.as_str())
    }
}

impl Platform {
    fn as_str(&self) -> &'static str {
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
        write!(f, "{}_{}", self.0.as_str(), self.1)
    }
}

impl re_framework::EntityId for ConversationId {
    fn get_id_string(&self) -> String {
        self.to_string()
    }
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PostSend {
    Nothing,
    CallTools {
        tool_rounds: usize,
        tool_calls: Vec<ToolCall>,
    },
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
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub note: String,
    pub addressee: String,
}

impl SystemMessage {
    pub fn to_content(&self) -> String {
        format!("[Reminder — IMPORTANT] For {}: {}", self.addressee, self.note)
    }
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
            attachments: self.attachments.iter().map(Attachment::dehydrated).collect(),
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
        for attachment in &self.attachments {
            match attachment {
                Attachment::Image { image, url, .. } => match image.hydrated_bytes() {
                    Some(b) => {
                        parts.push(format!("url: {url}"));
                        parts.push(marker.to_string());
                        bytes.push(b);
                    }
                    None => dehydrated += 1,
                },
                Attachment::File { filename, content_type, url } => {
                    let kind = content_type.as_deref().unwrap_or("unknown type");
                    parts.push(format!("file {filename} ({kind}) url: {url}"));
                }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pending {
    Message(ConversationMessage),
    System(SystemMessage),
}


#[derive(Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub pending: Vec<Pending>,
    pub state: ConversationState,
    pub last_transition: DateTime<Utc>,
    pub is_group: bool,
    pub bot_identity: String,
    #[serde(default)]
    pub compaction_in_flight: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMInput {
    ConversationMessage(ConversationMessage),
                SystemMessage(Vec<SystemMessage>),
    ToolResults(Vec<ToolResult>, Option<ConversationMessage>),
}

impl LLMInput {
    pub fn redacted(&self) -> LLMInput {
        match self {
            LLMInput::ConversationMessage(m) => LLMInput::ConversationMessage(m.redacted()),
            LLMInput::SystemMessage(batch) => LLMInput::SystemMessage(batch.clone()),
            LLMInput::ToolResults(results, user) => LLMInput::ToolResults(
                results.clone(),
                user.as_ref().map(ConversationMessage::redacted),
            ),
        }
    }

    pub fn messages_and_media(
        &self,
        marker: &str,
        full: bool,
    ) -> (Vec<ChatMessage>, Vec<Arc<Vec<u8>>>) {
        match self {
            LLMInput::ConversationMessage(msg) => {
                let (content, bytes) = msg.content_and_media(marker);
                (vec![ChatMessage::user(content)], bytes)
            }
            LLMInput::SystemMessage(batch) => {
                let content = batch
                    .iter()
                    .map(SystemMessage::to_content)
                    .collect::<Vec<_>>()
                    .join("\n");
                (vec![ChatMessage::user(content)], Vec::new())
            }
            LLMInput::ToolResults(results, user_msg) => {
                let mut messages: Vec<ChatMessage> = Vec::new();
                let mut bytes: Vec<Arc<Vec<u8>>> = Vec::new();

                for r in results {
                    let mut parts: Vec<String> = Vec::new();
                    let text = if full { &r.data.actual } else { &r.data.simplified };
                    if !text.is_empty() {
                        parts.push(text.clone());
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
                    messages.push(ChatMessage::tool(r.call.id.clone(), parts.join("\n")));
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
    ReadFile {
        path: String,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    EditFile {
        path: String,
        old_string: String,
        new_string: String,
    },
    SetReminder {
        delay_seconds: i64,
        note: String,
        addressee: String,
    },
    MetaNoOpExtraTurn,
    MetaMalformed {
        report: String,
    },
}

impl ToolType {
    pub fn rescue_timeout(&self) -> Duration {
        let ms = match self {
            ToolType::RunBashCommand { .. } => 300_000,
            ToolType::VisitUrl { .. } => 90_000,
            ToolType::WebSearch { .. } | ToolType::ResetBashContainer => 60_000,
            ToolType::ViewImage { .. }
            | ToolType::ReadFile { .. }
            | ToolType::EditFile { .. }
            | ToolType::SetReminder { .. }
            | ToolType::MetaNoOpExtraTurn
            | ToolType::MetaMalformed { .. } => 30_000,
        };
        Duration::milliseconds(ms)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool_type: ToolType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call: ToolCall,
    pub data: ToolResultData,
}

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
pub enum InterruptionReason {
    TimedOut,
    Failed,
    MalformedToolCall,
}

impl InterruptionReason {
    pub fn note(&self) -> &'static str {
        match self {
            InterruptionReason::TimedOut => "[Your previous turn timed out before it completed.]",
            InterruptionReason::Failed => {
                "[Your previous turn failed with an internal error and did not complete.]"
            }
            InterruptionReason::MalformedToolCall => {
                "[Your previous tool call could not be parsed, so the turn was dropped — check your tool-call format.]"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HistoryId(uuid::Uuid);

impl HistoryId {
    pub fn new() -> Self {
        HistoryId(uuid::Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: HistoryId,
    pub kind: HistoryEntryKind,
}

impl HistoryEntry {
    fn with_kind(kind: HistoryEntryKind) -> Self {
        HistoryEntry {
            id: HistoryId::new(),
            kind,
        }
    }
    pub fn input(input: LLMInput) -> Self {
        Self::with_kind(HistoryEntryKind::Input(input))
    }
    pub fn output(response: LLMResponse) -> Self {
        Self::with_kind(HistoryEntryKind::Output(response))
    }
    pub fn interrupted(reason: InterruptionReason) -> Self {
        Self::with_kind(HistoryEntryKind::OutputInterrupted(reason))
    }
    pub fn summary(summary: String) -> Self {
        Self::with_kind(HistoryEntryKind::Summary(summary))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HistoryEntryKind {
    Input(LLMInput),
    Output(LLMResponse),
    OutputInterrupted(InterruptionReason),
    Summary(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionOutput {
    pub summary: String,
    pub through: HistoryId,
}

pub fn latest_file_hash<'a>(history: &'a [HistoryEntry], path: &str) -> Option<&'a str> {
    history.iter().rev().find_map(|entry| match &entry.kind {
        HistoryEntryKind::Input(LLMInput::ToolResults(results, _)) => {
            results.iter().rev().find_map(|r| match &r.call.tool_type {
                ToolType::ReadFile { path: p, .. } | ToolType::EditFile { path: p, .. }
                    if p == path =>
                {
                    r.data.metadata.get("file_hash").map(String::as_str)
                }
                _ => None,
            })
        }
        _ => None,
    })
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ConversationAction {
    NewMessage {
        msg: String,
        user_id: String,
        name: String,
        attachments: Vec<Attachment>,
    },
    LLMDecisionResult(Result<LLMResponse, InterruptionReason>),
    MessageSent(Result<(), String>),
    ToolResult {
        id: String,
        result: Result<ToolResultData, String>,
    },
    CompactionResult(Result<CompactionOutput, InterruptionReason>),
    ReminderFired {
        note: String,
        addressee: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResultData {
    pub actual: String,
    pub simplified: String,
    pub image_for_assistant: Option<MessageImage>,
    pub metadata: HashMap<String, String>,
}

impl ToolResultData {
    pub fn text(actual: String, simplified: String) -> Self {
        ToolResultData {
            actual,
            simplified,
            image_for_assistant: None,
            metadata: HashMap::new(),
        }
    }
}

impl std::fmt::Debug for ConversationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversationAction::NewMessage { .. } => write!(f, "NewMessage"),
            ConversationAction::LLMDecisionResult(_) => write!(f, "LLMDecisionResult"),
            ConversationAction::MessageSent(_) => write!(f, "MessageSent"),
            ConversationAction::ToolResult { .. } => write!(f, "ToolResult"),
            ConversationAction::CompactionResult(_) => write!(f, "CompactionResult"),
            ConversationAction::ReminderFired { .. } => write!(f, "ReminderFired"),
        }
    }
}
