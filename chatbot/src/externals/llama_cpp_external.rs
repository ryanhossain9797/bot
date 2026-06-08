use crate::{
    types::conversation::{
        latest_is_group, HistoryEntry, LLMInput, LLMResponse, RecentConversation, ToolCall,
        ToolType, ConversationAction,
    },
    services::llama_cpp::LlamaCppService,
    Env,
};
use serde_json::Value;

use std::sync::Arc;

/// A plain-text budget note for the most recent tool result, escalating in three tiers so the model
/// isn't scared into quitting early: at ~50% nudge it to vary its approach (NOT to stop — an early
/// "wrap up" makes it bail prematurely), at ~80% tell it to start wrapping up, and at the cap a
/// synthesis directive (where tools are turned off, so it can't call another and this nudges it to
/// answer from history rather than stall). `None` below the halfway mark. Relative thresholds track
/// the cap if retuned. Not `<system-reminder>`: Qwen has no special handling for that tag, so a
/// plain bracketed marker is used instead. Lives here in the message stream — never the system turn
/// — to keep the cached system prefix stable.
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

/// Build the conversation as an OpenAI-style messages JSON array (without the system turn — the
/// agent prepends that). Each tool result carries its own `tool_call_id` (from the call it answers),
/// so no positional threading is needed. Prior reasoning is not replayed (Qwen3 guidance). When the
/// new input is a tool result, the running budget reminder is appended to it at render time (never
/// persisted, so old turns don't accumulate stale counts).
fn build_conversation(
    new_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
) -> Value {
    let history = maybe_recent_conversation
        .map(|rc| rc.history)
        .unwrap_or_default();

    let mut messages: Vec<Value> = Vec::new();

    for entry in &history {
        match entry {
            HistoryEntry::Input(input) => messages.extend(input.to_openai_messages()),
            HistoryEntry::Output(response) => messages.push(response.to_openai_message()),
        }
    }

    let mut new_messages = new_input.to_openai_messages();
    // Append the budget note to the last tool-result message of the batch. A user interjection
    // folded into the turn trails the results, so target the last `tool` message specifically
    // rather than the last message overall (which would land the note on the user's words).
    if matches!(new_input, LLMInput::ToolResults(..)) {
        if let Some(note) = budget_note(tool_rounds, max_tool_rounds) {
            if let Some(last_tool) = new_messages
                .iter_mut()
                .rev()
                .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
            {
                if let Some(content) = last_tool.get("content").and_then(|c| c.as_str()) {
                    last_tool["content"] = Value::String(format!("{content}\n\n{note}"));
                }
            }
        }
    }
    messages.extend(new_messages);

    Value::Array(messages)
}

/// Every tool call from a parsed assistant message, as `(id, name, arguments_json_string)` each.
/// The model may batch several calls in one turn; we run them all.
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
    bot_identity: &str,
) -> anyhow::Result<LLMResponse> {
    // DM-vs-group context for system-prompt selection: the latest message wins. Use the current
    // input's flag when it's a message (or carries a folded message); otherwise (a tool-result
    // continuation) read the most recent message from history.
    let is_group = match current_input {
        LLMInput::ConversationMessage(m) => m.is_group,
        LLMInput::ToolResults(_, Some(m)) => m.is_group,
        LLMInput::ToolResults(_, None) => maybe_recent_conversation
            .as_ref()
            .map(|rc| latest_is_group(&rc.history))
            .unwrap_or(false),
    };

    let conversation = build_conversation(
        current_input,
        maybe_recent_conversation,
        tool_rounds,
        max_tool_rounds,
    );

    println!("\n\n------------------------ NEW ITERATION ------------------------\n\n");
    println!(
        "{}",
        serde_json::to_string_pretty(&conversation).unwrap_or_default()
    );

    let parsed = llama_cpp
        .get_primary_response(conversation, allow_tools, is_group, bot_identity)
        .await?;

    let thoughts = parsed
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Straight from JSON to Option — absent / null / blank content is None, never a "" round-trip.
    // The model also emits the literal "<empty>" to deliberately stay silent (easier for it than
    // producing nothing at all); map that to None too, so it flows into the silent path.
    let message = parsed
        .get("content")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("<empty>"))
        .map(String::from);

    // message and tool calls are independent — a turn may carry either or both. Binding failures
    // surface as a failed decision. Sort calls by id so the assistant calls and (later) their
    // results share one canonical order in history, keeping the positional render aligned.
    let calls = all_tool_calls(&parsed);
    let mut tool_calls = Vec::with_capacity(calls.len());
    for (id, name, arguments) in calls {
        let tool_type = ToolType::bind(&name, &arguments)?;
        tool_calls.push(ToolCall { id, tool_type });
    }
    tool_calls.sort_by(|a, b| a.id.cmp(&b.id));

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
) -> ConversationAction {
    // Budget spent → final call with tools off, so the model can't emit another tool call; the
    // matching synthesis directive on the last tool result (see `budget_note`) nudges it to answer
    // from what it already gathered.
    let allow_tools = tool_rounds < max_tool_rounds;
    println!(
        "[tool budget] {tool_rounds}/{max_tool_rounds} tool calls this turn (tools {})",
        if allow_tools { "on" } else { "off — synthesizing" }
    );

    // The bot's own identity in the same name+id form used for message prefixes / mentions, so the
    // model can recognize when an id refers to itself.
    let bot_identity = format!("{} (id:{})", env.bot_name, env.bot_user_id);

    let llama_cpp_result = get_response_from_llm(
        env.llama_cpp.as_ref(),
        &current_input,
        maybe_recent_conversation,
        tool_rounds,
        max_tool_rounds,
        allow_tools,
        &bot_identity,
    )
    .await;

    match llama_cpp_result {
        Ok(llama_cpp_response) => ConversationAction::LLMDecisionResult(Ok(llama_cpp_response)),
        Err(err) => ConversationAction::LLMDecisionResult(Err(err.to_string())),
    }
}
