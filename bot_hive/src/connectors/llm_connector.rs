use crate::{
    models::user::{HistoryEntry, LLMDecisionType, LLMInput, UserAction},
    services::llm::LlmService,
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
    llm: &LlmService,
    current_input: &LLMInput,
    history: &[HistoryEntry],
) -> anyhow::Result<LLMResponse> {
    let mut ctx = llm.new_context()?;

    let dynamic_prompt = build_dynamic_prompt(current_input, history);

    let base_token_count = llm.load_base_prompt(&mut ctx)?;

    let total_tokens = llm.append_prompt(&mut ctx, &dynamic_prompt, base_token_count)?;

    let mut sampler = llm.create_sampler();

    let mut batch = LlmService::new_batch();
    let mut generated_tokens = Vec::new();

    let batch_tokens = total_tokens - base_token_count;
    let mut last_batch_idx = batch_tokens as i32 - 1;
    let mut n_cur = total_tokens;

    for _ in 0..LlmService::get_max_generation_tokens() {
        let new_token = sampler.sample(&ctx, last_batch_idx);

        if llm.is_eog_token(new_token) {
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
        if let Ok(output) = llm.token_to_str(*token, Special::Tokenize) {
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
    let llm_result = get_response_from_llm(env.llm.as_ref(), &current_input, &history).await;

    eprintln!("[DEBUG] llm_result: {:#?}", llm_result);

    match llm_result {
        Ok(llm_response) => UserAction::LLMDecisionResult(Ok(llm_response.outcome)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
