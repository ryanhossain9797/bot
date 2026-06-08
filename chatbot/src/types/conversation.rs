use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::Display;
use strum::{EnumDiscriminants, EnumIter};

/// Per-result snippet cap in web search output. Generous — SearxNG snippets rarely exceed a few
/// hundred chars, so this is effectively "don't truncate" with a safety net (was 20, far too tight
/// for the current model/context).
pub const MAX_SEARCH_DESCRIPTION_LENGTH: usize = 2000;

/// Max tool calls the model may make in a single user turn before the loop is cut short. Single
/// source of truth: the state machine enforces it; the system prompt discloses it.
pub const MAX_TOOL_ROUNDS: usize = 10;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub struct ConversationId(pub Platform, pub String);

impl Display for ConversationId {
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
pub enum ConversationState {
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
        /// Tool rounds so far this turn, carried through so that if `outcome` holds tool calls
        /// (a preamble + tools), the subsequent `handle_outcome` dispatch keeps the real count
        /// instead of resetting the `MAX_TOOL_ROUNDS` budget.
        tool_rounds: usize,
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
impl Default for ConversationState {
    fn default() -> Self {
        ConversationState::Idle {
            recent_conversation: None,
        }
    }
}

/// One inbound message in a conversation. `text` already carries the sender's name prefix
/// (`"Alice: hello"`) — the Discord adapter prepends it for every message, DM or group, so the
/// model always knows who is speaking (in a group, every human still maps to OpenAI `role: "user"`,
/// so the name in the text is the only speaker signal). `name`/`user_id` keep the same identity in
/// structured form. `is_group` is the DM-vs-group context of the message (latest message wins; not
/// persisted on the `Conversation`). `queued` is true when it was buffered into `pending` while the
/// bot was busy, so it crossed an in-flight response — surfaced as a `[Followup]` tag at render time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub text: String,
    pub queued: bool,
    /// Sender's platform user id (stable identity); empty for synthetic/system messages.
    pub user_id: String,
    /// Sender's display name; empty for synthetic/system messages. Already baked into `text`.
    pub name: String,
    /// Whether this message came from a group chat (vs a 1:1 DM). Drives system-prompt selection.
    pub is_group: bool,
}

impl ConversationMessage {
    /// Prompt content: the text (which already includes the `name:` prefix), tagged with
    /// `[Followup]` when it was queued mid-response so the model knows it may already be addressed.
    pub fn to_content(&self) -> String {
        if self.queued {
            format!("[Followup] {}", self.text)
        } else {
            self.text.clone()
        }
    }
}

/// The per-conversation entity the state machine drives: its current `state`, any `pending`
/// updates buffered while busy, and when it last transitioned. Keyed by [`ConversationId`] (a
/// channel/DM on some [`Platform`]) — one independent instance per conversation.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Conversation {
    pub pending: Vec<ConversationMessage>,
    pub state: ConversationState,
    pub last_transition: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LLMInput {
    ConversationMessage(ConversationMessage),
    /// One turn's batch of tool results (id order), each tagged with the call it answers.
    /// The optional trailing `ConversationMessage` is input that arrived mid-tool-run, folded into this
    /// same turn *after* the results — OpenAI requires tool responses to immediately follow the
    /// assistant's `tool_calls`, so the user message comes last.
    ToolResults(Vec<ToolResult>, Option<ConversationMessage>),
}

/// The `is_group` of the most recent message in `history` (latest message wins; defaults to DM /
/// `false` when there's no message to read). Used to pick the DM-vs-group system prompt and to give
/// synthetic messages (e.g. the timeout goodbye) the conversation's group-ness.
pub fn latest_is_group(history: &[HistoryEntry]) -> bool {
    history
        .iter()
        .rev()
        .find_map(|e| match e {
            HistoryEntry::Input(LLMInput::ConversationMessage(m)) => Some(m.is_group),
            HistoryEntry::Input(LLMInput::ToolResults(_, Some(m))) => Some(m.is_group),
            _ => None,
        })
        .unwrap_or(false)
}

impl LLMInput {
    /// Render to OpenAI messages: a user message is one message; a tool-result batch is one `tool`
    /// message per result (the template groups them into a single tool-response turn).
    pub fn to_openai_messages(&self) -> Vec<Value> {
        match self {
            LLMInput::ConversationMessage(msg) => vec![json!({ "role": "user", "content": msg.to_content() })],
            LLMInput::ToolResults(results, user_msg) => {
                let mut messages: Vec<Value> = results
                    .iter()
                    .map(|r| json!({ "role": "tool", "tool_call_id": r.id, "content": r.data.actual }))
                    .collect();
                // A user interjection that arrived mid-tool-run trails the results (protocol order).
                if let Some(msg) = user_msg {
                    messages.push(json!({ "role": "user", "content": msg.to_content() }));
                }
                messages
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

/// One decision from the model. `message` and `tool_calls` are independent: a turn may carry a
/// user-facing message, a batch of tool calls, or both (e.g. "Let me look that up…" + the calls).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMResponse {
    /// Reasoning (`<think>`); kept for logging but never replayed into the next prompt.
    pub thoughts: String,
    /// Conversation-facing text, if the model produced any.
    pub message: Option<String>,
    /// Tool calls to dispatch this turn; empty if none.
    pub tool_calls: Vec<ToolCall>,
}

impl LLMResponse {
    /// A degenerate decision: no user-facing message and no tool calls — i.e. the model chose to
    /// stay silent. Used to render an explicit silent marker (see `to_openai_message`).
    pub fn is_empty(&self) -> bool {
        self.message.as_deref().map_or(true, str::is_empty) && self.tool_calls.is_empty()
    }

    pub fn to_openai_message(&self) -> Value {
        // Assistant turn: the message (if any) as content, plus a native tool_calls array when
        // present. name/arguments are derived back from each bound ToolType (the inverse of `bind`).
        //
        // A silent turn (empty message, no tools — the model chose not to reply, common in groups)
        // is stored faithfully as `message: None`, but rendered here with an explicit marker so the
        // model can see in later turns that it deliberately passed (matters for [Followup] context).
        // The marker is render-time only — it is NEVER written back into stored history.
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
                    // Surface the folded-in interjection so recall/memory text stays faithful.
                    match user_msg {
                        Some(msg) => format!("{assistant}\nUser:\n{}", msg.text),
                        None => assistant,
                    }
                }
            },
            HistoryEntry::Output(LLMResponse { message, tool_calls, .. }) => {
                // Surface message and any calls so recall/memory stays faithful.
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

#[derive(Clone, Serialize)]
pub enum ConversationAction {
    ForceReset,
    NewMessage {
        /// Message text with the sender's name already prefixed (`"Alice: hello"`).
        msg: String,
        /// Sender's platform user id and display name (for the structured `ConversationMessage`).
        user_id: String,
        name: String,
        /// Whether it arrived from a group chat (vs a 1:1 DM).
        is_group: bool,
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
