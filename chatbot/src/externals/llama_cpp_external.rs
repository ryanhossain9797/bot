use crate::{
    models::user::{
        HistoryEntry, LLMDecisionType, LLMInput, LLMResponse, RecentConversation, ToolCall,
        ToolType, UserAction,
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
            HistoryEntry::Input(input) => messages.push(input.to_openai_message()),
            HistoryEntry::Output(response) => messages.push(response.to_openai_message()),
        }
    }

    let mut new_message = new_input.to_openai_message();
    if matches!(new_input, LLMInput::ToolResult { .. }) {
        if let Some(note) = budget_note(tool_rounds, max_tool_rounds) {
            if let Some(content) = new_message.get("content").and_then(|c| c.as_str()) {
                new_message["content"] = Value::String(format!("{content}\n\n{note}"));
            }
        }
    }
    messages.push(new_message);

    Value::Array(messages)
}

/// First tool call from a parsed assistant message, as `(id, name, arguments_json_string)`.
///
/// Single-call by design: the state machine runs one tool per turn (`parallel_tool_calls: false`).
/// Multi-tool is deferred to the state machine — if the model batches calls we take the first and
/// warn rather than drop the rest silently.
fn first_tool_call(parsed: &Value) -> Option<(String, String, String)> {
    let calls = parsed.get("tool_calls").and_then(|v| v.as_array())?;

    if calls.len() > 1 {
        println!(
            "[warn] model emitted {} tool calls; running only the first (multi-tool not yet supported)",
            calls.len()
        );
    }

    let call = calls.first()?;
    let id = call
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("call_0")
        .to_string();
    let name = call
        .pointer("/function/name")
        .and_then(|v| v.as_str())?
        .to_string();
    let arguments = call
        .pointer("/function/arguments")
        .and_then(|v| v.as_str())
        .unwrap_or("{}")
        .to_string();

    Some((id, name, arguments))
}

async fn get_response_from_llm(
    llama_cpp: &LlamaCppService,
    current_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
    allow_tools: bool,
) -> anyhow::Result<LLMResponse> {
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
        .get_primary_response(conversation, allow_tools)
        .await?;

    let thoughts = parsed
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // A tool call takes precedence over text; binding failures surface as a failed decision.
    if let Some((id, name, arguments)) = first_tool_call(&parsed) {
        let tool_type = ToolType::bind(&name, &arguments)?;
        return Ok(LLMResponse {
            thoughts,
            output: LLMDecisionType::IntermediateToolCall {
                tool_call: ToolCall { id, tool_type },
            },
        });
    }

    let content = parsed
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(LLMResponse {
        thoughts,
        output: LLMDecisionType::MessageUser { response: content },
    })
}

pub async fn get_llm_decision(
    env: Arc<Env>,
    current_input: LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
    tool_rounds: usize,
    max_tool_rounds: usize,
) -> UserAction {
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
    )
    .await;

    match llama_cpp_result {
        Ok(llama_cpp_response) => UserAction::LLMDecisionResult(Ok(llama_cpp_response)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
