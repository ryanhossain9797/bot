use crate::{
    types::conversation::{
        HistoryEntry, LLMInput, LLMResponse, RecentConversation, ToolCall,
        ToolType, ConversationAction,
    },
    services::llama_cpp::LlamaCppService,
    Env,
};
use chrono::Utc;
use llama_cpp_2::mtmd::mtmd_default_marker;
use serde_json::{json, Value};

use std::sync::Arc;

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

fn session_context_block(
    is_group: bool,
    bot_identity: &str,
    tool_rounds: usize,
    max_tool_rounds: usize,
) -> Value {
    let setting = if is_group {
        "GROUP CHAT (multiple participants)"
    } else {
        "DIRECT MESSAGE (one-to-one with the user)"
    };
    let now = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");

    let mut lines = vec![
        "=== SESSION CONTEXT (authoritative; current as of now) ===".to_string(),
        format!("Your username in this conversation: {bot_identity}"),
        format!("Setting: {setting}"),
        format!("Current time: {now}"),
    ];
    if let Some(note) = budget_note(tool_rounds, max_tool_rounds) {
        lines.push(note);
    }
    lines.push(
        "When the image, a tool result, or the user's correction contradicts what you remember, trust the source and update — don't defend your prior.".to_string(),
    );

    if is_group {
        lines.push(
            "Reminders: in a group you default to silence — chime in when your id is @mentioned, or occasionally on your own if you genuinely add something; otherwise reply with exactly `<empty>`. Match the @mention id, not the name. You are Terminal Alpha Beta.".to_string(),
        );
    } else {
        lines.push("Reminder: you are Terminal Alpha Beta.".to_string());
    }

    json!({ "role": "user", "content": lines.join("\n") })
}

fn build_conversation(
    new_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
    is_group: bool,
    bot_identity: &str,
) -> (Value, Vec<Arc<Vec<u8>>>) {
    let history = maybe_recent_conversation
        .map(|rc| rc.history)
        .unwrap_or_default();

    let marker = mtmd_default_marker();
    let mut messages: Vec<Value> = Vec::new();
    let mut images: Vec<Arc<Vec<u8>>> = Vec::new();

    for entry in &history {
        match entry {
            HistoryEntry::Input(input) => {
                let (msgs, bytes) = input.messages_and_media(marker);
                messages.extend(msgs);
                images.extend(bytes);
            }
            HistoryEntry::Output(response) => messages.push(response.to_openai_message()),
        }
    }

    let (live_msgs, live_bytes) = new_input.messages_and_media(marker);
    messages.extend(live_msgs);
    images.extend(live_bytes);

    messages.push(session_context_block(
        is_group,
        bot_identity,
        tool_rounds,
        max_tool_rounds,
    ));

    (Value::Array(messages), images)
}

fn all_tool_calls(parsed: &Value) -> Vec<(String, String, String)> {
    let Some(calls) = parsed.get("tool_calls").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    calls
        .iter()
        .filter_map(|call| {
            let id = call
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("call_0")
                .to_string();
            let name = call.pointer("/function/name").and_then(|v| v.as_str())?.to_string();
            let arguments = call
                .pointer("/function/arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}")
                .to_string();
            Some((id, name, arguments))
        })
        .collect()
}

async fn get_response_from_llm(
    llama_cpp: &LlamaCppService,
    current_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
    allow_tools: bool,
    is_group: bool,
    bot_identity: &str,
) -> anyhow::Result<LLMResponse> {
    let (conversation, images) = build_conversation(
        current_input,
        maybe_recent_conversation,
        tool_rounds,
        max_tool_rounds,
        is_group,
        bot_identity,
    );

    println!("\n\n------------------------ NEW ITERATION ------------------------\n\n");
    println!(
        "{}",
        serde_json::to_string_pretty(&conversation).unwrap_or_default()
    );

    if !images.is_empty() {
        println!("[image] feeding {} image(s) to the model", images.len());
    }

    let parsed = llama_cpp
        .get_primary_response(conversation, images, allow_tools)
        .await?;

    let thoughts = parsed
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let content = parsed
        .get("content")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    let explicit_empty = content.eq_ignore_ascii_case("<empty>");
    let message = (!content.is_empty() && !explicit_empty).then(|| content.to_string());

    let calls = all_tool_calls(&parsed);
    let mut tool_calls = Vec::with_capacity(calls.len());
    for (id, name, arguments) in calls {
        let tool_type = ToolType::bind(&name, &arguments)?;
        tool_calls.push(ToolCall { id, tool_type });
    }
    tool_calls.sort_by(|a, b| a.id.cmp(&b.id));

    if message.is_none() && tool_calls.is_empty() && !explicit_empty {
        eprintln!(
            "[llm] implicit empty response — model produced no message and no tool calls; nothing will be sent"
        );
    }

    Ok(LLMResponse {
        thoughts,
        message,
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
) -> ConversationAction {
    let allow_tools = tool_rounds < max_tool_rounds;
    println!(
        "[tool budget] {tool_rounds}/{max_tool_rounds} tool calls this turn (tools {})",
        if allow_tools { "on" } else { "off — synthesizing" }
    );

    let llama_cpp_result = get_response_from_llm(
        env.llama_cpp.as_ref(),
        &current_input,
        maybe_recent_conversation,
        tool_rounds,
        max_tool_rounds,
        allow_tools,
        is_group,
        &bot_identity,
    )
    .await;

    match llama_cpp_result {
        Ok(llama_cpp_response) => ConversationAction::LLMDecisionResult(Ok(llama_cpp_response)),
        Err(err) => {
            eprintln!("[llm] decision failed: {err}");
            ConversationAction::LLMDecisionResult(Err(err.to_string()))
        }
    }
}
