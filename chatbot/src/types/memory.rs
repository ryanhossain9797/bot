use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::conversation::{CompactionOutput, ConversationId, HistoryEntry, InterruptionReason};

#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryManager {
    pub state: MemoryManagerState,
    pub last_transition: DateTime<Utc>,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum MemoryManagerState {
    Idle,
    Compacting,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum MemoryManagerAction {
    Compact { history: Vec<HistoryEntry> },
    CompactionDone(Result<CompactionOutput, InterruptionReason>),
}

impl std::fmt::Debug for MemoryManagerAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryManagerAction::Compact { .. } => write!(f, "Compact"),
            MemoryManagerAction::CompactionDone(Ok(_)) => write!(f, "CompactionDone(ok)"),
            MemoryManagerAction::CompactionDone(Err(_)) => write!(f, "CompactionDone(err)"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryManagerConstructor {
    pub id: ConversationId,
}

impl re_framework::Identified for MemoryManagerConstructor {
    type Id = ConversationId;
    fn get_id(&self) -> &ConversationId {
        &self.id
    }
}
