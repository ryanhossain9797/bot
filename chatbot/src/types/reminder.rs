use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt::Display;

use crate::types::conversation::ConversationId;

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
    pub conversation_id: ConversationId,
    pub addressee: String,
    pub note: String,
    pub created_on: DateTime<Utc>,
    pub fire_at: DateTime<Utc>,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ReminderState {
    Pending,
    Fired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ReminderAction {
    Fire,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReminderConstructor {
    pub id: ReminderForConversationId,
    pub addressee: String,
    pub note: String,
    pub fire_at: DateTime<Utc>,
}

impl re_framework::Identified for ReminderConstructor {
    type Id = ReminderForConversationId;
    fn get_id(&self) -> &ReminderForConversationId {
        &self.id
    }
}
