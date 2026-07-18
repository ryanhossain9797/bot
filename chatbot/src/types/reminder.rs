use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::Display;

use crate::types::conversation::ConversationId;

/// Upper bound on how far out a reminder may be scheduled (~1 year). Bounds the
/// requested delay well below any chrono arithmetic overflow, so computing
/// `fire_at` can never panic.
pub const MAX_REMINDER_SECS: i64 = 366 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReminderId(uuid::Uuid);

impl ReminderId {
    pub fn new() -> Self {
        ReminderId(uuid::Uuid::new_v4())
    }
}

impl Default for ReminderId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for ReminderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Compound id so a conversation can hold multiple concurrent reminders (the
/// memory manager, keyed by bare `ConversationId`, is one-per-conversation and
/// does not work here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReminderForConversationId {
    pub conversation_id: ConversationId,
    pub reminder_id: ReminderId,
}

impl re_framework::EntityId for ReminderForConversationId {
    fn get_id_string(&self) -> String {
        format!("{}__{}", self.conversation_id, self.reminder_id)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ReminderForConversation {
    pub state: ReminderState,
    /// Where the fired reminder is delivered back to.
    pub conversation_id: ConversationId,
    /// Who set the reminder / who it is for (named in the fired turn — matters
    /// in a group chat).
    pub user_id: String,
    pub name: String,
    pub note: String,
    pub created_on: DateTime<Utc>,
    pub fire_at: DateTime<Utc>,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ReminderState {
    Pending,
    Fired,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ReminderAction {
    Fire,
}

impl std::fmt::Debug for ReminderAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReminderAction::Fire => write!(f, "Fire"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReminderConstructor {
    pub id: ReminderForConversationId,
    pub user_id: String,
    pub name: String,
    pub note: String,
    pub delay_seconds: i64,
}

impl re_framework::Identified for ReminderConstructor {
    type Id = ReminderForConversationId;
    fn get_id(&self) -> &ReminderForConversationId {
        &self.id
    }
}
