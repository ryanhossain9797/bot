use std::sync::Arc;

use crate::{
    models::user::{HistoryEntry, InternalFunctionResultData, UserAction},
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

    let actual = recent_history
        .iter()
        .map(|entry| entry.format_simplified())
        .collect::<Vec<_>>()
        .join("\n");

    let simplified = {
        let start_index = if recent_history.len() > 3 {
            recent_history.len() - 3
        } else {
            0
        };

        recent_history[start_index..]
            .iter()
            .map(|entry| entry.format_simplified())
            .collect::<Vec<_>>()
            .join("\n")
    };

    UserAction::InternalFunctionResult(Ok(InternalFunctionResultData {
        actual: format!(
            "SHORT TERM RECALL: Recent conversation history:\n{}",
            actual
        ),
        simplified: format!(
            "SHORT TERM RECALL: Recent conversation history:\n{}",
            simplified
        ),
    }))
}
