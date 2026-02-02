use crate::{
    models::user::{
        FunctionCall, HistoryEntry, InternalFunctionResultData, LLMDecisionType, LLMInput,
        LLMResponse, RecentConversation, ToolCall, ToolResultData, UserAction,
    },
    services::llama_cpp::LlamaCppService,
    Env,
};
use anyhow::anyhow;
use serde::Deserialize;
use serde_json;

use std::{
    iter::{self},
    sync::Arc,
};

fn format_input(input: &LLMInput) -> String {
    match input {
        LLMInput::UserMessage(msg) => {
            format!("User:\n{msg}")
        }
        LLMInput::InternalFunctionResult(InternalFunctionResultData { actual, .. })
        | LLMInput::ToolResult(ToolResultData { actual, .. }) => format!("Assistant:\n{actual}"),
    }
}

fn build_dynamic_prompt(
    new_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
) -> String {
    let (_prev_thoughts, history) = maybe_recent_conversation
        .map(|rc| (rc.thoughts, rc.history))
        .unwrap_or_else(|| ("NULL".to_string(), Vec::new()));

    let new_input = format_input(new_input);

    let conversation = history
        .iter()
        .map(|h| h.format_simplified())
        .chain(iter::once(new_input))
        .collect::<Vec<_>>()
        .join("\n\n");

    // Add the below bit back in if needed
    // Your previous thoughts were
    // {prev_thoughts}

    format!(
        r#"

Conversation

{conversation}

Assistant:
    "#
    )
}

#[derive(Debug, Clone, Deserialize)]
enum FlatLLMDecision {
    MessageUser(String),
    GetWeather(String),
    WebSearch(String),
    VisitUrl(String),
    RecallShortTerm(String),
    RecallLongTerm(String),
}

async fn get_response_from_llm(
    llama_cpp: &LlamaCppService,
    current_input: &LLMInput,
    maybe_recent_conversation: Option<RecentConversation>,
) -> anyhow::Result<LLMResponse> {
    let dynamic_prompt = build_dynamic_prompt(current_input, maybe_recent_conversation);

    println!("\n\n------------------------ NEW ITERATION ------------------------\n\n");

    println!("{dynamic_prompt}");

    let response = llama_cpp.get_thinking_response(&dynamic_prompt).await?;

    println!("[DEBUG MAIN RESPONSE]: {response}");

    let mut parts = response.splitn(2, "output:");

    let before = parts.next().ok_or(anyhow!("Missing thoughts section"))?;
    let after = parts.next().ok_or(anyhow!("Missing output section"))?;

    let thoughts = before
        .trim()
        .strip_prefix("thoughts:")
        .ok_or(anyhow!("Missing 'thoughts:' prefix"))?
        .trim()
        .to_string();

    let simple_output = after.trim().to_string();

    let executor_prompt = format!(
        r#"
    Input: {simple_output}
    Output:"#
    );
    let executor_response = llama_cpp.get_executor_response(&executor_prompt).await?;

    println!("\n\n-- EXECUTOR OUTPUT --\n\n");

    println!("Executor agent: {executor_response}");

    let decision_dto: FlatLLMDecision =
        serde_json::from_str(&executor_response).expect("should parse");

    let output: LLMDecisionType = match decision_dto {
        FlatLLMDecision::MessageUser(response) => LLMDecisionType::MessageUser { response },
        FlatLLMDecision::GetWeather(location) => LLMDecisionType::IntermediateToolCall {
            tool_call: ToolCall::GetWeather { location },
        },
        FlatLLMDecision::WebSearch(query) => LLMDecisionType::IntermediateToolCall {
            tool_call: ToolCall::WebSearch { query },
        },
        FlatLLMDecision::VisitUrl(url) => LLMDecisionType::IntermediateToolCall {
            tool_call: ToolCall::VisitUrl { url },
        },
        FlatLLMDecision::RecallShortTerm(reason) => LLMDecisionType::InternalFunctionCall {
            function_call: FunctionCall::RecallShortTerm { reason },
        },
        FlatLLMDecision::RecallLongTerm(search_term) => LLMDecisionType::InternalFunctionCall {
            function_call: FunctionCall::RecallLongTerm { search_term },
        },
    };

    Ok(LLMResponse {
        thoughts,
        output,
        simple_output,
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
