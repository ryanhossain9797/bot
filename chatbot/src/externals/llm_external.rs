//! The conversation's LLM external: builds an Anthropic Messages request from the history,
//! runs it on whichever backend `Env::llm` is (native llama.cpp or remote), and maps the reply
//! back to a `ConversationAction`. Like every external it never fails — errors become
//! `LLMDecisionResult(Err(..))`.

use crate::{
    llm::{
        anthropic::{Message, MessagesRequest, MessagesResponse},
        ModelRole,
    },
    prompts::{PRIMARY_MAX_TOKENS, PRIMARY_SYSTEM_PROMPT, PRIMARY_TEMPERATURE},
    types::conversation::{
        ConversationAction, HistoryEntryKind, InterruptionReason, LLMInput, LLMResponse, Platform,
        RecentConversation, Reply, ToolCall, ToolType,
    },
    Env,
};
use re_framework::Timestamp;

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
    let now = Timestamp::now().strftime("%Y-%m-%d %H:%M:%S UTC");
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
        "Only narrate before a tool call when there's genuinely new information to share (e.g. why you're trying something unusual or what you learned). Hold off for at least 5 tool calls in a row before adding commentary, and keep it brief."
            .to_string(),
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

fn build_request(
    new_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
    allow_tools: bool,
    is_group: bool,
    bot_identity: &str,
    platform: &Platform,
) -> MessagesRequest {
    let history = maybe_recent_conversation
        .map(|rc| rc.history)
        .unwrap_or_default();

    let mut messages: Vec<Message> = Vec::new();
    for entry in &history {
        match &entry.kind {
            HistoryEntryKind::Input(input) => messages.extend(input.to_messages(false)),
            HistoryEntryKind::Output(response) => messages.push(response.to_message()),
            HistoryEntryKind::OutputInterrupted(reason) => {
                messages.push(Message::assistant_text(reason.note()))
            }
            HistoryEntryKind::Summary(summary) => messages.push(Message::assistant_text(format!(
                "[Summary of earlier conversation, condensed to save context]\n{summary}"
            ))),
        }
    }
    messages.extend(new_input.to_messages(true));

    MessagesRequest {
        max_tokens: PRIMARY_MAX_TOKENS,
        system: Some(PRIMARY_SYSTEM_PROMPT.to_string()),
        messages,
        tools: if allow_tools {
            ToolType::tool_definitions()
        } else {
            Vec::new()
        },
        temperature: Some(PRIMARY_TEMPERATURE),
        footer: Some(session_footer(
            is_group,
            bot_identity,
            platform,
            tool_rounds,
            max_tool_rounds,
        )),
    }
}

fn to_llm_response(response: &MessagesResponse) -> LLMResponse {
    let thoughts = response.thinking();
    let content = response.text();
    let tool_calls: Vec<ToolCall> = response
        .tool_uses()
        .map(|(id, name, input)| {
            let id = id.to_string();
            match ToolType::bind(name, &input.to_string()) {
                Ok(tool_type) => ToolCall { id, tool_type },
                Err(e) => {
                    eprintln!("[llm] tool call did not parse: {e:#}");
                    ToolCall {
                        id,
                        tool_type: ToolType::MetaMalformed {
                            report: malformed_report(name, &e),
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

    LLMResponse {
        thoughts,
        reply,
        tool_calls,
    }
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

    let request = build_request(
        &current_input,
        maybe_recent_conversation,
        tool_rounds,
        max_tool_rounds,
        allow_tools,
        is_group,
        &bot_identity,
        &platform,
    );

    match Arc::clone(&env.llm)
        .complete(ModelRole::Primary, request)
        .await
    {
        Ok(response) => ConversationAction::LLMDecisionResult(Ok(to_llm_response(&response))),
        Err(e) => {
            eprintln!("[llm] {e:#}");
            ConversationAction::LLMDecisionResult(Err(InterruptionReason::Failed))
        }
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
