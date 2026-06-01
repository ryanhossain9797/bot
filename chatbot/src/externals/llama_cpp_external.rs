use crate::{
    models::user::{
        HistoryEntry, InternalFunctionResultData, LLMDecisionType, LLMInput, LLMResponse,
        NativeToolCall, RecentConversation, ToolCall, ToolResultData, UserAction,
    },
    services::llama_cpp::LlamaCppService,
    Env,
};
use serde_json::{json, Value};

use std::sync::Arc;

/// Append the OpenAI message(s) for one LLM input. A `tool` result references the id of the
/// preceding assistant tool call (`last_tool_call_id`); internal-function results (dormant — not
/// native tools) are surfaced as a user turn.
fn append_input(messages: &mut Vec<Value>, input: &LLMInput, last_tool_call_id: &Option<String>) {
    match input {
        LLMInput::UserMessage(msg) => messages.push(json!({ "role": "user", "content": msg })),
        LLMInput::ToolResult(ToolResultData { actual, .. }) => {
            let id = last_tool_call_id
                .clone()
                .unwrap_or_else(|| "call_0".to_string());
            messages.push(json!({ "role": "tool", "tool_call_id": id, "content": actual }));
        }
        LLMInput::InternalFunctionResult(InternalFunctionResultData { actual, .. }) => {
            messages.push(json!({ "role": "user", "content": actual }))
        }
    }
}

/// Append the OpenAI assistant message for one LLM output. A tool-call turn is rendered with a
/// native `tool_calls` array (content empty, matching how the model emits it); a plain reply is a
/// normal assistant message.
fn append_output(messages: &mut Vec<Value>, response: &LLMResponse) {
    match &response.output {
        LLMDecisionType::MessageUser { response: text } => {
            messages.push(json!({ "role": "assistant", "content": text }))
        }
        LLMDecisionType::IntermediateToolCall { .. } => match &response.tool_call {
            Some(NativeToolCall {
                id,
                name,
                arguments,
            }) => messages.push(json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": arguments }
                }]
            })),
            // Shouldn't happen (a tool-call decision always carries its native call); render an
            // empty assistant turn rather than fabricate one.
            None => messages.push(json!({ "role": "assistant", "content": "" })),
        },
        // Internal functions aren't native tools; dormant in the current cut.
        LLMDecisionType::InternalFunctionCall { function_call } => {
            messages.push(json!({ "role": "assistant", "content": format!("{function_call:?}") }))
        }
    }
}

/// Build the conversation as an OpenAI-style messages JSON array (WITHOUT the system turn — the
/// agent prepends that). Assistant tool-call turns and their `tool` results are reconstructed in
/// native form; the `tool_call_id` is threaded from each assistant call to the result that follows
/// it. Prior reasoning is intentionally NOT replayed (Qwen3 guidance).
fn build_conversation(
    new_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
) -> Value {
    let history = maybe_recent_conversation
        .map(|rc| rc.history)
        .unwrap_or_default();

    let mut messages: Vec<Value> = Vec::new();
    let mut last_tool_call_id: Option<String> = None;

    for entry in &history {
        match entry {
            HistoryEntry::Input(input) => append_input(&mut messages, input, &last_tool_call_id),
            HistoryEntry::Output(response) => {
                append_output(&mut messages, response);
                last_tool_call_id = response.tool_call.as_ref().map(|tc| tc.id.clone());
            }
        }
    }
    append_input(&mut messages, new_input, &last_tool_call_id);

    Value::Array(messages)
}

/// Pull the first tool call out of a parsed assistant message, if any. Returns
/// `(id, name, arguments_json_string)`.
fn first_tool_call(parsed: &Value) -> Option<(String, String, String)> {
    let call = parsed
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .and_then(|calls| calls.first())?;

    let id = call
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("call_0")
        .to_string();
    let name = call
        .pointer("/function/name")
        .and_then(|v| v.as_str())?
        .to_string();
    // `arguments` is a JSON string, e.g. "{\"city\":\"Paris\"}".
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
) -> anyhow::Result<LLMResponse> {
    let conversation = build_conversation(current_input, maybe_recent_conversation);

    println!("\n\n------------------------ NEW ITERATION ------------------------\n\n");
    println!(
        "{}",
        serde_json::to_string_pretty(&conversation).unwrap_or_default()
    );

    let parsed = llama_cpp.get_primary_response(conversation).await?;

    let thoughts = parsed
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // A tool call takes precedence over any text. Bind the raw JSON arguments to a typed model and
    // map to our `ToolCall`; a binding failure surfaces as a failed decision (never a panic).
    if let Some((id, name, arguments)) = first_tool_call(&parsed) {
        let tool_call = ToolCall::bind(&name, &arguments)?;
        return Ok(LLMResponse {
            thoughts,
            output: LLMDecisionType::IntermediateToolCall { tool_call },
            tool_call: Some(NativeToolCall {
                id,
                name,
                arguments,
            }),
        });
    }

    // Otherwise it's a plain reply to the user.
    let content = parsed
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(LLMResponse {
        thoughts,
        output: LLMDecisionType::MessageUser { response: content },
        tool_call: None,
    })
}

pub async fn get_llm_decision(
    env: Arc<Env>,
    current_input: LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
) -> UserAction {
    let llama_cpp_result = get_response_from_llm(
        env.llama_cpp.as_ref(),
        &current_input,
        maybe_recent_conversation,
    )
    .await;

    match llama_cpp_result {
        Ok(llama_cpp_response) => UserAction::LLMDecisionResult(Ok(llama_cpp_response)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
