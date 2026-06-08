use crate::types::conversation::{HistoryEntry, ToolResultData};

pub fn recall_short(history: &[HistoryEntry]) -> ToolResultData {
    let start_index = history.len().saturating_sub(20);
    let recent_history = &history[start_index..];

    let actual = recent_history
        .iter()
        .map(|entry| entry.format_simplified())
        .collect::<Vec<_>>()
        .join("\n");

    let simplified = {
        let start = recent_history.len().saturating_sub(3);
        recent_history[start..]
            .iter()
            .map(|entry| entry.format_simplified())
            .collect::<Vec<_>>()
            .join("\n")
    };

    ToolResultData {
        actual: format!("SHORT TERM RECALL: Recent conversation history:\n{actual}"),
        simplified: format!("SHORT TERM RECALL: Recent conversation history:\n{simplified}"),
    }
}
