use crate::{
    chat_format::ChatMessage,
    roles::{PrimaryRole, RenderInputs, Role},
    types::conversation::{
        ConversationAction, HistoryEntryKind, InterruptionReason, LLMInput, LLMResponse, Platform,
        RecentConversation, Reply, ToolCall, ToolType,
    },
    Env,
};
use chrono::Utc;
use llama_cpp_2::mtmd::mtmd_default_marker;

use std::sync::Arc;

fn malformed_report(name: &str, error: &anyhow::Error) -> String {
    let mut report = format!(
        "That tool call was rejected and did NOT run: {error}. Re-check the tool name and its \
         arguments against the tools available to you, then call it again."
    );
    if name.contains("attach") {
        report.push_str(
            " (Sending a file or image to the user is NOT a tool — instead put the marker \
             [[attach_file:PATH]] or [[attach_image:PATH]] in your reply text.)",
        );
    }
    report
}

fn budget_note(tool_rounds: usize, max_tool_rounds: usize) -> Option<String> {
    if max_tool_rounds == 0 {
        None
    } else if tool_rounds >= max_tool_rounds {
        Some(
            "[Tool budget exhausted — you've used all your tool-call turns and no more are \
             available. Answer the user now using the information already gathered above, and \
             clearly state anything you could not determine.]"
                .to_string(),
        )
    } else if tool_rounds * 5 >= max_tool_rounds * 4 {
        Some(format!(
            "[Tool budget: {tool_rounds}/{max_tool_rounds} used — you're running low. Either wrap up and answer with what you have, or make your remaining calls count: use them only if they'll meaningfully improve your answer.]"
        ))
    } else if tool_rounds * 2 >= max_tool_rounds {
        Some(format!(
            "[Tool budget: {tool_rounds}/{max_tool_rounds} used — if your current approach isn't working, try a different angle rather than repeating similar calls. You still have room to keep investigating.]"
        ))
    } else {
        None
    }
}

fn session_footer(
    is_group: bool,
    bot_identity: &str,
    platform: &Platform,
    tool_rounds: usize,
    max_tool_rounds: usize,
) -> String {
    let setting = if is_group {
        "GROUP CHAT (multiple participants)"
    } else {
        "DIRECT MESSAGE (one-to-one with the user), not a group chat- every message is meant for you, there is no one else."
    };
    let now = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let bot_name = crate::BOT_NAME;

    let mut lines = vec![
        "=== SYSTEM GENERATED CONVERSATION METADATA FOOTER ===".to_string(),
        format!("Your username in this conversation: {bot_identity}. Respond to both this name and {bot_name}"),
        format!("Setting: {setting}"),
        format!("Current time: {now}"),
        platform.formatting_note().to_string(),
    ];
    if let Some(note) = budget_note(tool_rounds, max_tool_rounds) {
        lines.push(note);
    }
    lines.push(
        "If a tool or the user contradicts your memory, your memory is likely wrong — verify with a tool when you can, then carry on with the corrected info; don't just agree or defend a wrong prior.".to_string(),
    );
    lines.push(
        "If you already have something worth sharing — a partial answer, or what you're about to do — say it right after your thinking and before the tool call, instead of calling silently.".to_string(),
    );
    lines.push(
        "The user cannot see tool results — you must send the information as a message."
            .to_string(),
    );

    if is_group {
        lines.push(
            format!("Reminders: in a group you default to silence — chime in when your id is @mentioned, or occasionally on your own if you genuinely add something; otherwise your whole reply must be the literal token [EMPTY] (exactly those seven characters with square brackets, nothing else — never (empty), empty, or any variant). Match the @mention id, not the name. You are {bot_name}."),
        );
    }

    lines.join("\n")
}

fn build_conversation(
    new_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
    is_group: bool,
    bot_identity: &str,
    platform: &Platform,
) -> (Vec<ChatMessage>, String, Vec<Arc<Vec<u8>>>) {
    let history = maybe_recent_conversation
        .map(|rc| rc.history)
        .unwrap_or_default();

    let marker = mtmd_default_marker();
    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut images: Vec<Arc<Vec<u8>>> = Vec::new();

    for entry in &history {
        match &entry.kind {
            HistoryEntryKind::Input(input) => {
                let (msgs, bytes) = input.messages_and_media(marker, false);
                messages.extend(msgs);
                images.extend(bytes);
            }
            HistoryEntryKind::Output(response) => messages.push(response.to_chat_message()),
            HistoryEntryKind::OutputInterrupted(reason) => {
                messages.push(ChatMessage::assistant(reason.note()))
            }
            HistoryEntryKind::Summary(summary) => messages.push(ChatMessage::assistant(format!(
                "[Summary of earlier conversation, condensed to save context]\n{summary}"
            ))),
        }
    }

    let (live_msgs, live_bytes) = new_input.messages_and_media(marker, true);
    messages.extend(live_msgs);
    images.extend(live_bytes);

    let footer = session_footer(
        is_group,
        bot_identity,
        platform,
        tool_rounds,
        max_tool_rounds,
    );

    (messages, footer, images)
}

async fn get_response_from_llm(
    role: Arc<PrimaryRole>,
    current_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
    allow_tools: bool,
    is_group: bool,
    bot_identity: &str,
    platform: &Platform,
) -> Result<LLMResponse, InterruptionReason> {
    let (messages, footer, images) = build_conversation(
        current_input,
        maybe_recent_conversation,
        tool_rounds,
        max_tool_rounds,
        is_group,
        bot_identity,
        platform,
    );

    println!("\n\n------------------------ NEW ITERATION ------------------------\n\n");
    println!(
        "{}",
        serde_json::to_string_pretty(&messages).unwrap_or_default()
    );
    println!("---- footer ----\n{footer}");

    if !images.is_empty() {
        println!("[image] feeding {} image(s) to the model", images.len());
    }

    let tools = allow_tools.then(ToolType::tool_definitions);
    let prompt = role
        .render_prompt(&RenderInputs {
            messages: &messages,
            tools: tools.as_deref(),
            footer: Some(&footer),
        })
        .map_err(|e| {
            eprintln!("[llm] prompt render failed: {e:#}");
            InterruptionReason::Failed
        })?;

    let raw = Arc::clone(&role).generate(prompt, images).await.map_err(|e| {
        eprintln!("[llm] generation failed: {e:#}");
        InterruptionReason::Failed
    })?;
    let parsed = role.parse_response(&raw);

    let thoughts = parsed.reasoning;
    let content = parsed.content;
    let tool_calls: Vec<ToolCall> = parsed
        .tool_calls
        .iter()
        .enumerate()
        .map(|(i, call)| {
            let id = format!("call_{i}");
            match ToolType::bind(&call.name, &call.arguments) {
                Ok(tool_type) => ToolCall { id, tool_type },
                Err(e) => {
                    eprintln!("[llm] tool call did not parse: {e:#}");
                    ToolCall {
                        id,
                        tool_type: ToolType::MetaMalformed {
                            report: malformed_report(&call.name, &e),
                        },
                    }
                }
            }
        })
        .collect();

    let explicit_empty = content.eq_ignore_ascii_case("[EMPTY]");
    let reply = if !content.is_empty() && !explicit_empty {
        Reply::Said(content)
    } else if explicit_empty || !tool_calls.is_empty() {
        Reply::Empty
    } else {
        Reply::Malformed
    };

    if matches!(reply, Reply::Malformed) {
        eprintln!(
            "[llm] malformed response — no message, no tool call, and no [EMPTY] token; nothing will be sent"
        );
    }

    Ok(LLMResponse {
        thoughts,
        reply,
        tool_calls,
    })
}

pub async fn get_llm_decision(
    env: Arc<Env>,
    current_input: LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
    is_group: bool,
    bot_identity: String,
    platform: Platform,
) -> ConversationAction {
    let allow_tools = tool_rounds < max_tool_rounds;
    println!(
        "[tool budget] {tool_rounds}/{max_tool_rounds} tool calls this turn (tools {})",
        if allow_tools {
            "on"
        } else {
            "off — synthesizing"
        }
    );

    let llama_cpp_result = get_response_from_llm(
        Arc::clone(&env.primary),
        &current_input,
        maybe_recent_conversation,
        tool_rounds,
        max_tool_rounds,
        allow_tools,
        is_group,
        &bot_identity,
        &platform,
    )
    .await;

    match llama_cpp_result {
        Ok(llama_cpp_response) => ConversationAction::LLMDecisionResult(Ok(llama_cpp_response)),
        Err(reason) => ConversationAction::LLMDecisionResult(Err(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::malformed_report;

    #[test]
    fn malformed_report_names_the_error_and_adds_attach_hint() {
        let unknown = anyhow::anyhow!("model called an unknown tool: attach_file");
        let report = malformed_report("attach_file", &unknown);
        assert!(report.contains("did NOT run"));
        assert!(report.contains("attach_file"));
        assert!(report.contains("[[attach_file:PATH]]"));

        let bad_args =
            anyhow::anyhow!("set_reminder arguments failed to bind: missing field delay_seconds");
        let report = malformed_report("set_reminder", &bad_args);
        assert!(report.contains("did NOT run"));
        assert!(report.contains("missing field delay_seconds"));
        assert!(!report.contains("[[attach_file"));
    }
}
