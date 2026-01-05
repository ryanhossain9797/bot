use crate::{
    models::user::{HistoryEntry, LLMDecisionType, LLMInput, UserAction},
    services::llama_cpp::LlamaCppService,
    Env,
};
use llama_cpp_2::model::Special;
use serde::Deserialize;
use std::{io::Write, sync::Arc};

fn format_history_entry(entry: &HistoryEntry) -> String {
    match entry {
        HistoryEntry::Input(input) => format_input(input, true),
        HistoryEntry::Output(output) => match output {
            LLMDecisionType::Final { response } => {
                format!("<|im_start|>assistant\n{}<|im_end|>", response)
            }
            LLMDecisionType::IntermediateToolCall {
                thoughts,
                maybe_intermediate_response,
                tool_call,
            } => {
                let mut lines = Vec::new();
                lines.push(format!("THOUGHTS: {}", thoughts));
                if let Some(resp) = maybe_intermediate_response {
                    lines.push(format!("INTERMEDIATE RESPONSE: {}", resp));
                }
                lines.push(format!("CALL TOOL: {:?}", tool_call));
                format!("<|im_start|>assistant\n{}<|im_end|>", lines.join("\n"))
            }
        },
    }
}

fn format_history(history: &[HistoryEntry]) -> String {
    history
        .iter()
        .map(format_history_entry)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_input(input: &LLMInput, truncate: bool) -> String {
    match input {
        LLMInput::UserMessage(msg) => {
            let mut content = msg.clone();
            if truncate && content.len() > crate::models::user::MAX_TOOL_OUTPUT_LENGTH {
                content.truncate(crate::models::user::MAX_TOOL_OUTPUT_LENGTH);
                content.push_str("... (truncated)");
            }
            format!("<|im_start|>user\n{}<|im_end|>", content)
        }
        LLMInput::ToolResult(result) => {
            let mut content = result.clone();
            if truncate && content.len() > crate::models::user::MAX_TOOL_OUTPUT_LENGTH {
                content.truncate(crate::models::user::MAX_TOOL_OUTPUT_LENGTH);
                content.push_str("... (truncated)");
            }
            format!("<|im_start|>user\n[TOOL RESULT]:\n{}<|im_end|>", content)
        }
    }
}

fn build_dynamic_prompt(current_input: &LLMInput, history: &[HistoryEntry]) -> String {
    let mut parts = Vec::new();

    let history_str = format_history(history);
    if !history_str.is_empty() {
        parts.push(history_str);
    }

    if let Some(HistoryEntry::Output(LLMDecisionType::IntermediateToolCall { thoughts, .. })) =
        history.last()
    {
        parts.push(format!(
            "<|im_start|>system\nREMINDER: Your current plan was:\n{}<|im_end|>",
            thoughts
        ));
    }

    parts.push(format_input(current_input, false));

    format!("\n{}\n<|im_start|>assistant\n", parts.join("\n\n"))
}

#[derive(Debug, Deserialize)]
struct LLMResponse {
    outcome: LLMDecisionType,
}

async fn get_response_from_llm(
    llama_cpp: &LlamaCppService,
    current_input: &LLMInput,
    history: &[HistoryEntry],
) -> anyhow::Result<LLMResponse> {
    let mut ctx = llama_cpp.new_context()?;
    println!("[DEBUG] llama Context created.");

    let dynamic_prompt = build_dynamic_prompt(current_input, history);

    let base_token_count = llama_cpp.load_base_prompt(&mut ctx)?;
    println!("[DEBUG] Base prompt loaded ({base_token_count} tokens).",);

    let (total_tokens, last_batch_size) =
        llama_cpp.append_prompt(&mut ctx, &dynamic_prompt, base_token_count)?;
    println!("[DEBUG] Dynamic prompt appended (Total tokens: {total_tokens}).");

    let mut sampler = llama_cpp.create_sampler();

    let mut batch = LlamaCppService::new_batch();
    let mut generated_tokens = Vec::new();

    let mut last_batch_idx = last_batch_size - 1;
    let mut n_cur = total_tokens;

    let max_generation_tokens = LlamaCppService::get_max_generation_tokens();
    println!("[DEBUG] Starting inference loop (max_tokens: {max_generation_tokens}).");
    for nth in 0..max_generation_tokens {
        let new_token = sampler.sample(&ctx, last_batch_idx);

        if llama_cpp.is_eog_token(new_token) {
            break;
        }

        generated_tokens.push(new_token);

        if nth % (max_generation_tokens / 4) == 0 && nth > 0 {
            let quarters = nth / (max_generation_tokens / 4);
            println!("{quarters}/4 of limit crossed, {nth} tokens");
            let _ = std::io::stdout().flush();
        }

        batch.clear();
        batch.add(new_token, n_cur as i32, &[0], true)?;

        ctx.decode(&mut batch)?;

        n_cur += 1;
        last_batch_idx = batch.n_tokens() - 1;
    }
    println!(
        "[DEBUG] Inference loop finished. Generated {} tokens.",
        generated_tokens.len()
    );

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
) -> UserAction {
    println!("[DEBUG] Starting get_llm_decision...");
    let llama_cpp_result =
        get_response_from_llm(env.llama_cpp.as_ref(), &current_input, &history).await;
    eprintln!("[DEBUG] llama_cpp_result: {:#?}", llama_cpp_result);

    match llama_cpp_result {
        Ok(llama_cpp_response) => UserAction::LLMDecisionResult(Ok(llama_cpp_response.outcome)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
