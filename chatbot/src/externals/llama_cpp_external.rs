use crate::{
    types::conversation::{
        HistoryEntry, LLMInput, LLMResponse, RecentConversation, ToolCall,
        ToolType, ConversationAction,
    },
    services::llama_cpp::LlamaCppService,
    Env,
};
use chrono::Utc;
use serde_json::{json, Value};

use std::sync::Arc;

/// A plain-text tool-budget note, escalating in three tiers so the model isn't scared into quitting
/// early: at ~50% nudge it to vary its approach (NOT to stop — an early "wrap up" makes it bail
/// prematurely), at ~80% tell it to start wrapping up, and at the cap a synthesis directive (where
/// tools are turned off, so it can't call another and this nudges it to answer from history rather
/// than stall). `None` below the halfway mark. Relative thresholds track the cap if retuned. Not
/// `<system-reminder>`: Qwen has no special handling for that tag, so a plain bracketed marker is
/// used instead. Emitted as a line in the SESSION CONTEXT block (`session_context_block`), never the
/// system turn — so the cached system prefix stays stable and there's one budget mechanism, not two.
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

/// The dynamic SESSION CONTEXT block: a `user`-role message carrying the volatile, per-conversation
/// facts the static system prompt promises (identity, group-vs-DM setting, current time) plus the
/// tool budget. Built fresh each turn and placed at the very end of the stream (see
/// `build_conversation`), so it's maximally recent and leaves the cached `[system][history]` prefix
/// untouched. Never persisted.
///
/// This is the single home for tool-budget messaging: `budget_note` (the escalating tier logic) is
/// emitted as one of these lines rather than separately attached to the last tool result. The block
/// is the very last message before the model's turn, so it's an even more recent place for that
/// nudge than the tool result was — and there's only one mechanism instead of two.
///
/// Role is `user`, not `system`, deliberately: the Qwen3 chat template **rejects** any `system`
/// message that isn't the leading one (it errors the render — verified with the `probe` crate), and
/// a trailing `system` turn isn't representable. A trailing `user` turn renders cleanly right before
/// the assistant turn and the model honors it (its reasoning cites "the session context"). The
/// `=== SESSION CONTEXT ===` header plus the static system prompt's description carry the authority;
/// the header also keeps it distinct from real participant turns (which are prefixed `Name (id:N):`).
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
        format!("Your identity: {bot_identity}"),
        format!("Setting: {setting}"),
        format!("Current time: {now}"),
    ];
    // Tool budget — only once it's worth mentioning (>= halfway; `budget_note` returns None below
    // that), so a fresh message with no tool use adds no "0/10" noise. The tier wording escalates
    // (vary approach -> wrap up -> exhausted).
    if let Some(note) = budget_note(tool_rounds, max_tool_rounds) {
        lines.push(note);
    }

    json!({ "role": "user", "content": lines.join("\n") })
}

/// Build the conversation as an OpenAI-style messages JSON array (without the static system turn —
/// the agent prepends that). Each tool result carries its own `tool_call_id` (from the call it
/// answers), so no positional threading is needed. Prior reasoning is not replayed (Qwen3 guidance).
/// The dynamic SESSION CONTEXT block is appended last, just before the model's turn (see
/// `session_context_block`); it also carries the tool-budget note, so nothing is attached to the
/// tool result itself and stale counts never accumulate in persisted history.
fn build_conversation(
    new_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
    is_group: bool,
    bot_identity: &str,
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

    messages.extend(new_input.to_openai_messages());

    // Dynamic context goes LAST — right before the model's turn — for maximum recency and to keep
    // the cached `[system][history]` prefix stable across turns. It carries the tool budget (via
    // `budget_note`), so there's no separate budget reminder attached to the tool result. It is
    // render-only: never written back into persisted history.
    messages.push(session_context_block(
        is_group,
        bot_identity,
        tool_rounds,
        max_tool_rounds,
    ));

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
    is_group: bool,
    bot_identity: &str,
) -> anyhow::Result<LLMResponse> {
    // DM-vs-group flag and the bot's own identity on this conversation's platform are conversation
    // facts (set once at construction, persisted on the state). They feed the dynamic SESSION
    // CONTEXT block that `build_conversation` appends at the tail of the stream.
    let conversation = build_conversation(
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

    let parsed = llama_cpp
        .get_primary_response(conversation, allow_tools)
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
    is_group: bool,
    bot_identity: String,
) -> ConversationAction {
    // Budget spent → final call with tools off, so the model can't emit another tool call; the
    // matching synthesis directive on the last tool result (see `budget_note`) nudges it to answer
    // from what it already gathered.
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
        Err(err) => ConversationAction::LLMDecisionResult(Err(err.to_string())),
    }
}
