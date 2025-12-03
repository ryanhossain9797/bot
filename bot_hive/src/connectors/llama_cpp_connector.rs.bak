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
    let history_json = serde_json::to_string_pretty(history).unwrap_or_else(|_| "[]".to_string());
    let history_section = format!("Conversation History (JSON):\n{}", history_json);

    let current_input_str = serialize_input(current_input);

    format!(
        "\n{}\n\n{}\n<|im_start|>assistant\n",
        history_section, current_input_str
    )
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

    let total_tokens = llama_cpp.append_prompt(&mut ctx, &dynamic_prompt, base_token_count)?;

    let mut sampler = llama_cpp.create_sampler();

    let mut batch = LlamaCppService::new_batch();
    let mut generated_tokens = Vec::new();

    let batch_tokens = total_tokens - base_token_count;
    let mut last_batch_idx = batch_tokens as i32 - 1;
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
    _env: Arc<Env>,
    _current_input: LLMInput,
    _history: Vec<HistoryEntry>,
) -> UserAction {
    // Llama.cpp connector disconnected - no longer functional
    // Base image doesn't have GGUF file
    // Use ollama_connector instead
    
    // OLD IMPLEMENTATION (commented out):
    // let llama_cpp_result = get_response_from_llm(env.llama_cpp.as_ref(), &current_input, &history).await;
    // eprintln!("[DEBUG] llama_cpp_result: {:#?}", llama_cpp_result);
    // match llama_cpp_result {
    //     Ok(llama_cpp_response) => UserAction::LLMDecisionResult(Ok(llama_cpp_response.outcome)),
    //     Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    // }
    
    eprintln!("[ERROR] llama_cpp_connector called but is disconnected - use ollama_connector instead");
    UserAction::LLMDecisionResult(Err(
        "Llama.cpp connector is disconnected - use Ollama instead".to_string()
    ))
}
