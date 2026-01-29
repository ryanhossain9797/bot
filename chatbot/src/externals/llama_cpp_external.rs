use crate::{
    models::user::{
        HistoryEntry, LLMDecisionType, LLMInput, UserAction, MAX_HISTORY_TEXT_LENGTH,
        MAX_INTERNAL_FUNCTION_OUTPUT_LENGTH, MAX_TOOL_OUTPUT_LENGTH,
    },
    services::llama_cpp::LlamaCppService,
    Env,
};
use llama_cpp_2::{
    llama_batch::LlamaBatch, model::Special, sampling::LlamaSampler, token::LlamaToken,
};
use serde::Deserialize;
use std::{io::Write, ops::ControlFlow, sync::Arc};

fn format_output(output: &LLMDecisionType) -> String {
    match output {
        LLMDecisionType::Final { response } => {
            let mut content = response.clone();
            if content.len() > MAX_HISTORY_TEXT_LENGTH {
                content.truncate(content.ceil_char_boundary(MAX_HISTORY_TEXT_LENGTH));
                content.push_str("... (truncated)");
            }
            format!("<|im_start|>assistant\n{}<|im_end|>", content)
        }
        LLMDecisionType::IntermediateToolCall {
            thoughts: _,
            progress_notification,
            tool_call,
        } => {
            let mut lines = Vec::new();

            if let Some(msg) = progress_notification {
                lines.push(format!("INTERMEDIATE PROGRESS: {}", msg));
            }
            lines.push(format!("CALL TOOL: {:?}", tool_call));
            format!("<|im_start|>assistant\n{}<|im_end|>", lines.join("\n"))
        }
        LLMDecisionType::InternalFunctionCall {
            thoughts: _,
            function_call,
        } => {
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
            HistoryEntry::Output(output) => format_output(output),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn build_dynamic_prompt(
    current_input: &LLMInput,
    history: &[HistoryEntry],
    truncate: bool,
) -> String {
    let mut parts = Vec::new();

    let history_str = format_history(history, truncate);
    if !history_str.is_empty() {
        parts.push(history_str);
    }

    if let Some(HistoryEntry::Output(
        LLMDecisionType::IntermediateToolCall { thoughts, .. }
        | LLMDecisionType::InternalFunctionCall { thoughts, .. },
    )) = history.last()
    {
        parts.push(format!(
            "<|im_start|>system\nREMINDER: Your current plan was:\n{}<|im_end|>",
            thoughts
        ));
    } else {
        parts.push("<|im_start|>system\nREMINDER: You have no current plan. Below is the conversation history<|im_end|>".to_string());
        let history_str = format_history(history, truncate);
        if !history_str.is_empty() {
            parts.push(history_str);
        }
    }

    parts.push(format_input(current_input, false));

    format!("\n{}\n<|im_start|>assistant\n", parts.join("\n\n"))
}

#[derive(Debug, Deserialize)]
struct LLMResponse {
    outcome: LLMDecisionType,
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
    history: &[HistoryEntry],
    truncate: bool,
) -> anyhow::Result<LLMResponse> {
    let mut ctx = llama_cpp.new_context()?;

    let dynamic_prompt = build_dynamic_prompt(current_input, history, truncate);

    let base_token_count = llama_cpp.load_base_prompt(&mut ctx)?;

    let (total_tokens, last_batch_size) =
        llama_cpp.append_prompt(&mut ctx, &dynamic_prompt, base_token_count)?;

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
    println!("[DEBUG] Generated tokens: {}.", generated_tokens.len());

    let mut response_bytes = Vec::new();
    for token in &generated_tokens {
        if let Ok(output) = llama_cpp.token_to_str(*token, Special::Tokenize) {
            response_bytes.extend_from_slice(output.as_bytes());
        }
    }
    let response = String::from_utf8_lossy(&response_bytes).to_string();

    print!("{}", response);
    println!();
    let _ = std::io::stdout().flush();

    let parsed_response: LLMResponse = serde_json::from_str(&response)?;

    Ok(parsed_response)
}

pub async fn get_llm_decision(
    env: Arc<Env>,
    current_input: LLMInput,
    history: Vec<HistoryEntry>,
    truncate_history: bool,
) -> UserAction {
    let llama_cpp_result = get_response_from_llm(
        env.llama_cpp.as_ref(),
        &current_input,
        &history,
        truncate_history,
    )
    .await;

    match llama_cpp_result {
        Ok(llama_cpp_response) => UserAction::LLMDecisionResult(Ok(llama_cpp_response.outcome)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
