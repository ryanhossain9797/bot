use crate::{
    models::user::{
        HistoryEntry, LLMDecisionType, LLMInput, LLMResponse, UserAction, MAX_HISTORY_TEXT_LENGTH,
        MAX_INTERNAL_FUNCTION_OUTPUT_LENGTH, MAX_TOOL_OUTPUT_LENGTH,
    },
    services::llama_cpp::LlamaCppService,
    Env,
};
use llama_cpp_2::{
    llama_batch::LlamaBatch, model::Special, sampling::LlamaSampler, token::LlamaToken,
};
use serde_json;

use std::{
    io::{self, Write},
    ops::ControlFlow,
    sync::Arc,
};

fn format_output(output: &LLMDecisionType) -> String {
    match output {
        LLMDecisionType::MessageUser { response } => {
            let mut content = response.clone();
            if content.len() > MAX_HISTORY_TEXT_LENGTH {
                content.truncate(content.ceil_char_boundary(MAX_HISTORY_TEXT_LENGTH));
                content.push_str("... (truncated)");
            }
            format!("<|im_start|>assistant\n{}<|im_end|>", content)
        }
        LLMDecisionType::IntermediateToolCall { tool_call } => {
            let mut lines = Vec::new();

            lines.push(format!("CALL TOOL: {:?}", tool_call));
            format!("<|im_start|>assistant\n{}<|im_end|>", lines.join("\n"))
        }
        LLMDecisionType::InternalFunctionCall { function_call } => {
            let mut lines = Vec::new();
            lines.push(format!("CALL INTERNAL FUNCTION: {:?}", function_call));
            format!("<|im_start|>assistant\n{}<|im_end|>", lines.join("\n"))
        }
    }
}

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

fn format_history(history: &[HistoryEntry], truncate: bool) -> String {
    history
        .iter()
        .map(|entry| match entry {
            HistoryEntry::Input(input) => format_input(input, truncate),
            HistoryEntry::Output(output) => format_output(&output.outcome),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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

fn build_dynamic_prompt(
    new_input: &LLMInput,
    maybe_last_thoughts: Option<String>,
    truncate: bool,
) -> String {
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

struct GenerationState {
    tokens: Vec<LlamaToken>,
    n_cur: usize,
    last_idx: i32,
    sampler: LlamaSampler,
    batch: LlamaBatch<'static>,
}

async fn get_response_from_llm(
    llama_cpp: &LlamaCppService,
    current_input: &LLMInput,
    maybe_last_thoughts: Option<String>,
    truncate: bool,
) -> anyhow::Result<LLMResponse> {
    print!("[DEBUG] ");
    let _ = io::stdout().flush();

    let mut ctx = llama_cpp.new_context()?;

    let dynamic_prompt = build_dynamic_prompt(current_input, maybe_last_thoughts, truncate);

    let base_token_count = llama_cpp.load_base_prompt(&mut ctx)?;

    let (total_tokens, last_batch_size) =
        llama_cpp.append_prompt(&mut ctx, &dynamic_prompt, base_token_count)?;

    print!("Total tokens: {total_tokens} ");
    let _ = io::stdout().flush();

    let initial_state = GenerationState {
        tokens: Vec::new(),
        n_cur: total_tokens,
        last_idx: last_batch_size - 1,
        sampler: llama_cpp.create_sampler(),
        batch: LlamaCppService::new_batch(),
    };

    let max_generation_tokens = LlamaCppService::get_max_generation_tokens();

    let result = (0..max_generation_tokens).try_fold(
        initial_state,
        |GenerationState {
             mut tokens,
             mut n_cur,
             mut last_idx,
             mut sampler,
             mut batch,
         },
         nth| {
            let token = sampler.sample(&ctx, last_idx);

            if let Ok(output) = llama_cpp.token_to_str(token, Special::Tokenize) {
                print!("{output}");
            }

            if llama_cpp.is_eog_token(token) {
                return ControlFlow::Break(Ok(tokens));
            }

            tokens.push(token);

            if nth > 0 && nth % (max_generation_tokens / 4) == 0 {
                println!(
                    "{}/4 of limit crossed ({} tokens)",
                    nth / (max_generation_tokens / 4),
                    nth
                );
            }

            match (|| -> anyhow::Result<()> {
                batch.clear();
                batch.add(token, n_cur as i32, &[0], true)?;
                ctx.decode(&mut batch)?;
                Ok(())
            })() {
                Ok(_) => {
                    n_cur += 1;
                    last_idx = batch.n_tokens() - 1;
                    ControlFlow::Continue(GenerationState {
                        tokens,
                        n_cur,
                        last_idx,
                        sampler,
                        batch,
                    })
                }
                Err(e) => ControlFlow::Break(Err(e)),
            }
        },
    );

    let generated_tokens = match result {
        ControlFlow::Continue(GenerationState { tokens, .. }) => Ok(tokens),
        ControlFlow::Break(res) => res,
    }?;
    print!("Generated tokens: {} ", generated_tokens.len());
    let _ = io::stdout().flush();

    let mut response_bytes = Vec::new();
    for token in &generated_tokens {
        if let Ok(output) = llama_cpp.token_to_str(*token, Special::Tokenize) {
            response_bytes.extend_from_slice(output.as_bytes());
        }
    }
    let response = String::from_utf8_lossy(&response_bytes).to_string();

    println!("\n{}\n", response);
    let _ = std::io::stdout().flush();

    let parsed_response: LLMResponse = serde_json::from_str(&response)?;

    Ok(parsed_response)
}

pub async fn get_llm_decision(
    env: Arc<Env>,
    current_input: LLMInput,
    maybe_last_thoughts: Option<String>,
    truncate_history: bool,
) -> UserAction {
    let llama_cpp_result = get_response_from_llm(
        env.llama_cpp.as_ref(),
        &current_input,
        maybe_last_thoughts,
        truncate_history,
    )
    .await;

    match llama_cpp_result {
        Ok(llama_cpp_response) => UserAction::LLMDecisionResult(Ok(llama_cpp_response)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
