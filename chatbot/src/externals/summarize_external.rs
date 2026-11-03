use std::sync::Arc;

use crate::Env;
use crate::llm::ModelRole;
use crate::llm::anthropic::{Message, MessagesRequest, MessagesResponse};
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

fn usable_summary(response: &MessagesResponse) -> Option<String> {
    let summary = response.text().trim().to_string();
    (!summary.is_empty()).then_some(summary)
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
        Ok(response) => match usable_summary(&response) {
            Some(summary) => {
                MemoryManagerAction::CompactionDone(Ok(CompactionOutput { summary, through }))
            }
            None => {
                eprintln!(
                    "[compact] summarizer returned no summary text (stop {:?}); keeping the history uncompacted",
                    response.stop_reason
                );
                MemoryManagerAction::CompactionDone(Err(InterruptionReason::Failed))
            }
        },
        Err(e) => {
            eprintln!("[compact] summarization failed: {e:#}");
            MemoryManagerAction::CompactionDone(Err(InterruptionReason::Failed))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::usable_summary;
    use crate::llm::anthropic::{ContentBlock, MessagesResponse, Role, StopReason, Usage};

    fn response(content: Vec<ContentBlock>, stop_reason: StopReason) -> MessagesResponse {
        MessagesResponse {
            id: "msg".into(),
            model: "m".into(),
            role: Role::Assistant,
            content,
            stop_reason: Some(stop_reason),
            usage: Usage::default(),
        }
    }

    #[test]
    fn thinking_only_reply_is_not_a_summary() {
        let reply = response(
            vec![ContentBlock::Thinking {
                thinking: "long think".into(),
                signature: String::new(),
            }],
            StopReason::MaxTokens,
        );
        assert_eq!(usable_summary(&reply), None);
        assert_eq!(usable_summary(&response(vec![ContentBlock::text("  \n ")], StopReason::EndTurn)), None);
    }

    #[test]
    fn text_is_the_summary_even_if_cut_off() {
        let reply = response(vec![ContentBlock::text(" Alice likes Rust. ")], StopReason::MaxTokens);
        assert_eq!(usable_summary(&reply).as_deref(), Some("Alice likes Rust."));
    }
}
