use std::sync::Arc;

use crate::Env;
use crate::llm::ModelRole;
use crate::llm::anthropic::{Message, MessagesRequest};
use crate::prompts::{SUMMARIZER_MAX_TOKENS, SUMMARIZER_SYSTEM_PROMPT, SUMMARIZER_TEMPERATURE};
use crate::types::conversation::{
    CompactionOutput, ConversationMessage, HistoryEntry, HistoryEntryKind, InterruptionReason,
    LLMInput,
};
use crate::types::media::Attachment;
use crate::types::memory_manager::MemoryManagerAction;

fn render_message(msg: &ConversationMessage) -> String {
    let mut line = format!("{}: {}", msg.name, msg.to_content());
    let images = msg
        .attachments
        .iter()
        .filter(|a| matches!(a, Attachment::Image { .. }))
        .count();
    let files = msg
        .attachments
        .iter()
        .filter(|a| matches!(a, Attachment::File { .. }))
        .count();
    if images > 0 {
        line.push_str(&format!(
            " [attached {images} image(s) — not visible to you]"
        ));
    }
    if files > 0 {
        line.push_str(&format!(" [attached {files} file(s)]"));
    }
    line
}

fn history_to_transcript(history: &[HistoryEntry]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for entry in history {
        match &entry.kind {
            HistoryEntryKind::Summary(summary) => {
                lines.push(format!("Summary of earlier conversation:\n{summary}"))
            }
            HistoryEntryKind::Input(LLMInput::ConversationMessage(msg)) => {
                lines.push(render_message(msg))
            }
            HistoryEntryKind::Input(LLMInput::SystemMessage(batch)) => {
                for sys in batch {
                    lines.push(format!(
                        "Reminder fired for {}: {}",
                        sys.addressee, sys.note
                    ));
                }
            }
            HistoryEntryKind::Input(LLMInput::ToolResults(results, followup)) => {
                for result in results {
                    let mut line = format!(
                        "Tool result [{}]: {}",
                        result.call.tool_type.wire_name(),
                        result.data.simplified
                    );
                    if result.data.image_for_assistant.is_some() {
                        line.push_str(
                            " [returned an image the assistant viewed — not visible to you]",
                        );
                    }
                    lines.push(line);
                }
                if let Some(msg) = followup {
                    lines.push(render_message(msg));
                }
            }
            HistoryEntryKind::Output(response) => {
                if let Some(message) = response.message() {
                    lines.push(format!("Assistant: {message}"));
                }
                for call in &response.tool_calls {
                    lines.push(format!(
                        "Assistant called tool: {}",
                        call.tool_type.wire_name()
                    ));
                }
            }
            HistoryEntryKind::OutputInterrupted(reason) => {
                lines.push(format!("Assistant: {}", reason.note()))
            }
        }
    }
    lines.join("\n")
}

pub async fn summarize(env: Arc<Env>, history: Vec<HistoryEntry>) -> MemoryManagerAction {
    if !env.llm.has(ModelRole::Utility) {
        eprintln!("[compact] utility model unavailable — skipping compaction");
        return MemoryManagerAction::CompactionDone(Err(InterruptionReason::Failed));
    }

    let Some(through) = history.last().map(|entry| entry.id.clone()) else {
        eprintln!("[compact] empty history — nothing to compact");
        return MemoryManagerAction::CompactionDone(Err(InterruptionReason::Failed));
    };

    let request = MessagesRequest {
        max_tokens: SUMMARIZER_MAX_TOKENS,
        system: Some(SUMMARIZER_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::user_text(history_to_transcript(&history))],
        tools: Vec::new(),
        temperature: Some(SUMMARIZER_TEMPERATURE),
        footer: None,
    };

    match env
        .llm
        .complete(ModelRole::Utility, request)
        .await
    {
        Ok(response) => {
            let summary = response.text().trim().to_string();
            MemoryManagerAction::CompactionDone(Ok(CompactionOutput { summary, through }))
        }
        Err(e) => {
            eprintln!("[compact] summarization failed: {e:#}");
            MemoryManagerAction::CompactionDone(Err(InterruptionReason::Failed))
        }
    }
}
