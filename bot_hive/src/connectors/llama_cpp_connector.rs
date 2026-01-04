use crate::{
    models::user::{HistoryEntry, LLMDecisionType, LLMInput, UserAction},
    services::llama_cpp::LlamaCppService,
    Env,
};
use llama_cpp_2::model::Special;
use serde::Deserialize;
use std::{io::Write, sync::Arc};

fn serialize_input(input: &LLMInput) -> String {
    match input {
        LLMInput::UserMessage(msg) => format!("<|im_start|>user\n{}<|im_end|>", msg),
        LLMInput::ToolResult(result) => {
            format!("<|im_start|>user\nTool Result: {}<|im_end|>", result)
        }
    }
}

fn build_dynamic_prompt(current_input: &LLMInput, history: &[HistoryEntry]) -> String {
    let mut parts = Vec::new();

    // Check if there's a thought from the previous turn and inject it as a system message
    if let Some(HistoryEntry::Output(LLMDecisionType::IntermediateToolCall { thoughts, .. })) =
        history.last()
    {
        parts.push(format!(
            "<|im_start|>system\nTHOUGHTS FROM PREVIOUS ACTION: {}<|im_end|>",
            thoughts
        ));
    }

    let history_json = serde_json::to_string_pretty(history).unwrap_or_else(|_| "[]".to_string());
    parts.push(format!("Conversation History (JSON):\n{}", history_json));

    let current_input_str = serialize_input(current_input);
    parts.push(current_input_str);

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

    let dynamic_prompt = build_dynamic_prompt(current_input, history);

    let base_token_count = llama_cpp.load_base_prompt(&mut ctx)?;

    let (total_tokens, last_batch_size) =
        llama_cpp.append_prompt(&mut ctx, &dynamic_prompt, base_token_count)?;

    let mut sampler = llama_cpp.create_sampler();

    let mut batch = LlamaCppService::new_batch();
    let mut generated_tokens = Vec::new();

    let mut last_batch_idx = last_batch_size - 1;
    let mut n_cur = total_tokens;

    for _ in 0..LlamaCppService::get_max_generation_tokens() {
        let new_token = sampler.sample(&ctx, last_batch_idx);

        if llama_cpp.is_eog_token(new_token) {
            break;
        }

        generated_tokens.push(new_token);

        batch.clear();
        batch.add(new_token, n_cur as i32, &[0], true)?;

        ctx.decode(&mut batch)?;

        n_cur += 1;
        last_batch_idx = batch.n_tokens() - 1;
    }

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
    let llama_cpp_result =
        get_response_from_llm(env.llama_cpp.as_ref(), &current_input, &history).await;
    eprintln!("[DEBUG] llama_cpp_result: {:#?}", llama_cpp_result);

    match llama_cpp_result {
        Ok(llama_cpp_response) => UserAction::LLMDecisionResult(Ok(llama_cpp_response.outcome)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
