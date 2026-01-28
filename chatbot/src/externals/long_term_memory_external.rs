use std::sync::Arc;

use crate::{
    models::user::{HistoryEntry, UserAction},
    Env,
};

pub async fn commit_to_memory(user_id: String, history: Vec<HistoryEntry>) -> UserAction {
    UserAction::CommitResult(Ok(()))
}
