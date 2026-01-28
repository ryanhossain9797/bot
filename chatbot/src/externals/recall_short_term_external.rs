use std::sync::Arc;

use crate::{
    models::user::{HistoryEntry, UserAction},
    Env,
};

pub async fn execute_short_recall(env: Arc<Env>, history: Vec<HistoryEntry>) -> UserAction {
    let _ = env;
    let start_index = if history.len() > 20 {
        history.len() - 20
    } else {
        0
    };
    let recent_history = &history[start_index..];

    let formatted_history = recent_history
        .iter()
        .map(|entry| entry.format())
        .collect::<Vec<_>>()
        .join("\n\n");

    UserAction::InternalFunctionResult(Ok(format!(
        "Recent conversation history (last 20 entries):\n\n{}",
        formatted_history
    )))
}
