use std::sync::Arc;

use crate::chat_format::ChatMessage;
use crate::roles::{RenderInputs, Role};
use crate::types::conversation::{HistoryEntry, InterruptionReason, LLMInput};
use crate::types::memory::MemoryManagerAction;
use crate::Env;

fn history_to_transcript(history: &[HistoryEntry]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for entry in history {
        match entry {
            HistoryEntry::Summary(summary) => {
                lines.push(format!("Summary of earlier conversation:\n{summary}"))
            }
            HistoryEntry::Input(LLMInput::ConversationMessage(msg)) => {
                lines.push(format!("{}: {}", msg.name, msg.to_content()))
            }
            HistoryEntry::Input(LLMInput::ToolResults(results, followup)) => {
                for result in results {
                    lines.push(format!(
                        "Tool result [{}]: {}",
                        result.call.tool_type.wire_name(),
                        result.data.simplified
                    ));
                }
                if let Some(msg) = followup {
                    lines.push(format!("{}: {}", msg.name, msg.to_content()));
                }
            }
            HistoryEntry::Output(response) => {
                if let Some(message) = response.message() {
                    lines.push(format!("Assistant: {message}"));
                }
                for call in &response.tool_calls {
                    lines.push(format!("Assistant called tool: {}", call.tool_type.wire_name()));
                }
            }
            HistoryEntry::OutputInterrupted(reason) => {
                lines.push(format!("Assistant: {}", reason.note()))
            }
        }
    }
    lines.join("\n")
}

pub async fn summarize(env: Arc<Env>, history: Vec<HistoryEntry>) -> MemoryManagerAction {
    let Some(utility) = env.utility.clone() else {
        eprintln!("[compact] utility model unavailable — skipping compaction");
        return MemoryManagerAction::CompactionDone(Err(InterruptionReason::Failed));
    };

    let transcript = history_to_transcript(&history);
    let prompt = match utility.render_prompt(&RenderInputs {
        messages: &[ChatMessage::user(transcript)],
        tools: None,
        footer: None,
    }) {
        Ok(prompt) => prompt,
        Err(e) => {
            eprintln!("[compact] prompt render failed: {e:#}");
            return MemoryManagerAction::CompactionDone(Err(InterruptionReason::Failed));
        }
    };

    let close_marker = utility.thinking().close_marker;
    match Arc::clone(&utility).generate(prompt, Vec::new()).await {
        Ok(raw) => {
            let summary = raw
                .split_once(&close_marker)
                .map_or(raw.as_str(), |(_, after)| after)
                .trim()
                .to_string();
            MemoryManagerAction::CompactionDone(Ok(summary))
        }
        Err(e) => {
            eprintln!("[compact] summarization failed: {e:#}");
            MemoryManagerAction::CompactionDone(Err(InterruptionReason::Failed))
        }
    }
}
