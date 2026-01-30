use crate::{
    models::user::{
        FunctionCall, LLMDecisionType, LLMInput, LLMResponse, MathOperation, ToolCall, UserAction,
        MAX_HISTORY_TEXT_LENGTH, MAX_INTERNAL_FUNCTION_OUTPUT_LENGTH, MAX_TOOL_OUTPUT_LENGTH,
    },
    services::llama_cpp::LlamaCppService,
    Env,
};
use anyhow::anyhow;
use serde::Deserialize;
use serde_json;

use std::sync::Arc;

fn format_input(input: &LLMInput, truncate: bool) -> String {
    match input {
        LLMInput::UserMessage(msg) => {
            let mut content = msg.clone();
            if truncate && content.len() > crate::models::user::MAX_HISTORY_TEXT_LENGTH {
                content.truncate(content.ceil_char_boundary(MAX_HISTORY_TEXT_LENGTH));
                content.push_str("... (truncated)");
            }
            format!("user said:\n\"{}\"", content)
        }
        LLMInput::InternalFunctionResult(result) => {
            let mut content = result.clone();
            if content.len() > MAX_INTERNAL_FUNCTION_OUTPUT_LENGTH {
                content.truncate(content.ceil_char_boundary(MAX_INTERNAL_FUNCTION_OUTPUT_LENGTH));
                content.push_str("... (truncated)");
            }

            format!("tool result\n{}", content)
        }
        LLMInput::ToolResult(result) => {
            let mut content = result.clone();
            if content.len() > MAX_TOOL_OUTPUT_LENGTH {
                content.truncate(content.ceil_char_boundary(MAX_TOOL_OUTPUT_LENGTH));
                content.push_str("... (truncated)");
            }

            format!("tool result:\n\"{}\"", content)
        }
    }
}

fn build_dynamic_prompt(new_input: &LLMInput, maybe_last_thoughts: Option<String>) -> String {
    let prev_thoughts = maybe_last_thoughts.unwrap_or("NULL".to_string());
    let new_input = format_input(new_input, false);

    format!(
        r#"

Previous thoughts:
{prev_thoughts}

New input:
{new_input}

IMPORTANT: Based on the previous thoughts and new information. Try to answer the user's question.
If you need more information call a different tool but prioritize answering the user if possible

Answer the user briefly without any unnecessary details. Don't try to be polite or conversational just state facts
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
    maybe_last_thoughts: Option<String>,
) -> anyhow::Result<LLMResponse> {
    let dynamic_prompt = build_dynamic_prompt(current_input, maybe_last_thoughts);

    println!("DYNAMIC: {dynamic_prompt}");

    let response = llama_cpp.get_thinking_response(&dynamic_prompt)?;

    println!("MAIN RESPONSE: {response}");

    let mut parts = response.splitn(2, "output:");

    let before = parts.next().ok_or(anyhow!("Missing thoughts section"))?;
    let after = parts.next().ok_or(anyhow!("Missing output section"))?;

    let thoughts = before
        .trim()
        .strip_prefix("thoughts:")
        .ok_or(anyhow!("Missing 'thoughts:' prefix"))?
        .trim()
        .to_string();

    let output = after.trim().to_string();

    println!("T: {thoughts}\nO: {output}");

    let executor_prompt = format!(
        r#"
    system

    if the input is message-user just generate MessageUser with the provided text
    for all other input run the tool with the provided parameters

    input: {output}
    "#
    );
    let executor_response = llama_cpp.get_executor_response(&executor_prompt)?;

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

    Ok(LLMResponse { thoughts, output })
}

pub async fn get_llm_decision(
    env: Arc<Env>,
    current_input: LLMInput,
    maybe_last_thoughts: Option<String>,
) -> UserAction {
    let llama_cpp_result =
        get_response_from_llm(env.llama_cpp.as_ref(), &current_input, maybe_last_thoughts).await;

    match llama_cpp_result {
        Ok(llama_cpp_response) => UserAction::LLMDecisionResult(Ok(llama_cpp_response)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
