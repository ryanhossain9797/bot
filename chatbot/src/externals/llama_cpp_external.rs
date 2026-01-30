use crate::{
    models::user::{
        LLMDecisionType, LLMInput, LLMResponse, UserAction, MAX_HISTORY_TEXT_LENGTH,
        MAX_INTERNAL_FUNCTION_OUTPUT_LENGTH, MAX_TOOL_OUTPUT_LENGTH,
    },
    services::llama_cpp::LlamaCppService,
    Env,
};
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
            format!("<|im_start|>user\n{}<|im_end|>", content)
        }
        LLMInput::InternalFunctionResult(result) => {
            let mut content = result.clone();
            if content.len() > MAX_INTERNAL_FUNCTION_OUTPUT_LENGTH {
                content.truncate(content.ceil_char_boundary(MAX_INTERNAL_FUNCTION_OUTPUT_LENGTH));
                content.push_str("... (truncated)");
            }

            format!(
                "<|im_start|>user\n[INTERNAL FUNCTION RESULT]:\n{}<|im_end|>",
                content
            )
        }
        LLMInput::ToolResult(result) => {
            let mut content = result.clone();
            if content.len() > MAX_TOOL_OUTPUT_LENGTH {
                content.truncate(content.ceil_char_boundary(MAX_TOOL_OUTPUT_LENGTH));
                content.push_str("... (truncated)");
            }

            format!("<|im_start|>user\n[TOOL RESULT]:\n{}<|im_end|>", content)
        }
    }
}

fn generate_llm_response_examples() -> String {
    use crate::models::user::{
        FunctionCall, LLMDecisionType, LLMResponse, MathOperation, ToolCall,
    };

    let mut examples = String::new();

    // Example 1: MessageUser
    let message_user_response = LLMResponse {
        thoughts: "...".to_string(),
        outcome: LLMDecisionType::MessageUser {
            response: "Hello there! How can I help you today?".to_string(),
        },
    };
    examples.push_str(&format!(
        "MessageUser Example:\n{}\n\n",
        serde_json::to_string_pretty(&message_user_response).unwrap()
    ));

    // Example 2: IntermediateToolCall - WebSearch
    let tool_call_websearch_response = LLMResponse {
        thoughts: "...".to_string(),
        outcome: LLMDecisionType::IntermediateToolCall {
            tool_call: ToolCall::WebSearch {
                query: "latest news headlines".to_string(),
            },
        },
    };
    examples.push_str(&format!(
        "IntermediateToolCall (WebSearch) Example:\n{}\n\n",
        serde_json::to_string(&tool_call_websearch_response).unwrap()
    ));

    // Example 3: IntermediateToolCall - MathCalculation
    let tool_call_math_response = LLMResponse {
        thoughts: "...".to_string(),
        outcome: LLMDecisionType::IntermediateToolCall {
            tool_call: ToolCall::MathCalculation {
                operations: vec![MathOperation::Add(5.0, 3.0), MathOperation::Mul(2.0, 4.0)],
            },
        },
    };
    examples.push_str(&format!(
        "IntermediateToolCall (MathCalculation) Example:\n{}\n\n",
        serde_json::to_string_pretty(&tool_call_math_response).unwrap()
    ));

    // Example 4: InternalFunctionCall - RecallShortTerm
    let internal_call_short_term_response = LLMResponse {
        thoughts: "...".to_string(),
        outcome: LLMDecisionType::InternalFunctionCall {
            function_call: FunctionCall::RecallShortTerm {
                reason: "User asked about previous topic.".to_string(),
            },
        },
    };
    examples.push_str(&format!(
        "InternalFunctionCall (RecallShortTerm) Example:\n{}\n\n",
        serde_json::to_string_pretty(&internal_call_short_term_response).unwrap()
    ));

    // Example 5: InternalFunctionCall - RecallLongTerm
    let internal_call_long_term_response = LLMResponse {
        thoughts: "...".to_string(),
        outcome: LLMDecisionType::InternalFunctionCall {
            function_call: FunctionCall::RecallLongTerm {
                search_term: "project details".to_string(),
            },
        },
    };
    examples.push_str(&format!(
        "InternalFunctionCall (RecallLongTerm) Example:\n{}\n\n",
        serde_json::to_string_pretty(&internal_call_long_term_response).unwrap()
    ));

    examples
}

fn build_dynamic_prompt(new_input: &LLMInput, maybe_last_thoughts: Option<String>) -> String {
    let llm_response_examples = generate_llm_response_examples();
    let prev_thoughts = if let Some(last_thoughts) = maybe_last_thoughts {
        print!("Thoughts from last turn: {} ", last_thoughts);
        format!("system\nTHOUGHTS:\n{last_thoughts}")
    } else {
        print!("Thoughts from last turn: null ");
        "system\nPREVIOUS THOUGHTS: NULL;".to_string()
    };
    let new_input = format_input(new_input, false);

    format!(
        r#"
    
    --- LLMResponse Examples ---

    {llm_response_examples}

    --- End LLMResponse Examples ---

    --- Thoughts from the previous iteration ---

    {prev_thoughts}

    --- End previous thoughts ---

    --- New input (User message or an outcome of previous thoughts) ---

    {new_input}

    --- End new input

    <|im_start|>assistant:
    "#
    )
}

async fn get_response_from_llm(
    llama_cpp: &LlamaCppService,
    current_input: &LLMInput,
    maybe_last_thoughts: Option<String>,
) -> anyhow::Result<LLMResponse> {
    let dynamic_prompt = build_dynamic_prompt(current_input, maybe_last_thoughts);
    let response = llama_cpp.get_thinking_response(&dynamic_prompt)?;

    let parsed_response: LLMResponse = serde_json::from_str(&response)?;

    // Also prompt the executor agent to respond with "PONG"
    let executor_prompt = "system\n get_weather dhaka\nagent: ";
    let executor_response = llama_cpp.get_executor_response(executor_prompt)?;

    println!("Executor agent: {executor_response}");

    let _executor_parsed: LLMDecisionType =
        serde_json::from_str(&executor_response).expect("should parse");

    Ok(parsed_response)
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
