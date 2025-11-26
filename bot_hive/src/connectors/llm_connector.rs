use std::{io::Write, num::NonZeroU32, sync::Arc};

use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
};
use serde::Deserialize;

use crate::models::user::{HistoryEntry, LLMDecisionType, LLMInput, UserAction};
use crate::Env;

/// Serialize the current input to a prompt string
fn serialize_input(input: &LLMInput) -> String {
    match input {
        LLMInput::UserMessage(msg) => format!("<|im_start|>user\n{}<|im_end|>", msg),
        LLMInput::ToolResult(result) => {
            format!("<|im_start|>user\nTool Result: {}<|im_end|>", result)
        }
    }
}

/// Builds the dynamic part of the prompt (history + current input)
fn build_dynamic_prompt(current_input: &LLMInput, history: &[HistoryEntry]) -> String {
    let history_json = serde_json::to_string_pretty(history).unwrap_or_else(|_| "[]".to_string());
    let history_section = format!("Conversation History (JSON):\n{}", history_json);

    let current_input_str = serialize_input(current_input);

    format!(
        "\n{}\n\n{}\n<|im_start|>assistant\n",
        history_section, current_input_str
    )
}

/// Builds the complete conversation prompt (static base + dynamic parts)
fn build_conversation_prompt(
    base_prompt: &str,
    current_input: &LLMInput,
    history: &[HistoryEntry],
) -> String {
    let dynamic_part = build_dynamic_prompt(current_input, history);
    format!("{}{}", base_prompt, dynamic_part)
}

#[derive(Debug, Deserialize)]
struct LLMResponse {
    outcome: LLMDecisionType,
}

async fn get_response_from_llm(
    llm: &(LlamaModel, LlamaBackend),
    session_path: &str,
    current_input: &LLMInput,
    history: &[HistoryEntry],
) -> anyhow::Result<LLMResponse> {
    let (model, backend) = llm;

    // Fixed low temperature to reduce creativity and make responses deterministic
    let temp = 0.1;

    // Increased context size to handle longer conversations and prevent NoKvCacheSlot errors
    // KV cache memory usage scales with context size, but modern models handle this well
    const CONTEXT_SIZE: u32 = 8192;
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(CONTEXT_SIZE))
        .with_n_threads(num_cpus::get() as i32)
        .with_n_threads_batch(num_cpus::get() as i32);

    let ctx_result = model.new_context(backend, ctx_params);

    match ctx_result {
        Ok(mut ctx) => {
            // Try to load session file - if it fails, fall back to full prompt
            let session_load_result = ctx.load_session_file(session_path, CONTEXT_SIZE as usize);

            let (base_tokens, dynamic_tokens) = match session_load_result {
                Ok(base_tokens) => {
                    // Session loaded successfully - only tokenize dynamic part
                    let dynamic_prompt = build_dynamic_prompt(current_input, history);
                    let dynamic_tokens = model.str_to_token(&dynamic_prompt, AddBos::Never)?;
                    (base_tokens, dynamic_tokens)
                }
                Err(e) => {
                    // Session load failed - use full prompt
                    eprintln!(
                        "Warning: Failed to load session file '{}': {}",
                        session_path, e
                    );
                    eprintln!("Falling back to full prompt evaluation (slower)");
                    let base_prompt = crate::external_connections::llm::BasePrompt::new();
                    let conversation_prompt =
                        build_conversation_prompt(base_prompt.as_str(), current_input, history);
                    let tokens = model.str_to_token(&conversation_prompt, AddBos::Always)?;
                    (vec![], tokens) // Empty base_tokens, full prompt in dynamic_tokens
                }
            };

            // Add tokens to batch (starting after base tokens if session was loaded)
            let mut batch = LlamaBatch::new(8192, 1);
            let start_pos = base_tokens.len() as i32;

            for (i, token) in dynamic_tokens.iter().enumerate() {
                let is_last = i == dynamic_tokens.len() - 1;
                let pos = start_pos + i as i32;
                batch.add(*token, pos, &[0], is_last)?;
            }

            ctx.decode(&mut batch)?;

            let grammar = include_str!("../../grammars/response.gbnf");

            let mut sampler = LlamaSampler::chain_simple([
                LlamaSampler::temp(temp),
                LlamaSampler::grammar(model, grammar, "root")
                    .expect("Failed to load grammar - check GBNF syntax"),
                LlamaSampler::dist(0),
            ]);

            // Increased max_tokens proportionally to context size
            // Still leaving plenty of room for history and base prompt
            let max_tokens = 2000;
            // Track absolute position: base + dynamic tokens
            let mut n_cur = (base_tokens.len() + dynamic_tokens.len()) as i32;
            let mut generated_tokens = Vec::new();
            let mut response_bytes = Vec::new();

            for _ in 0..max_tokens {
                let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);

                if model.is_eog_token(new_token) {
                    break;
                }

                generated_tokens.push(new_token);

                // Try to convert token to string for display (allow incomplete UTF-8)
                if let Ok(output) = model.token_to_str(new_token, Special::Tokenize) {
                    response_bytes.extend_from_slice(output.as_bytes());
                    // Use lossy conversion for real-time display
                    print!("{}", String::from_utf8_lossy(output.as_bytes()));
                    let _ = std::io::stdout().flush();
                }

                batch.clear();
                batch.add(new_token, n_cur, &[0], true)?;
                n_cur += 1;

                ctx.decode(&mut batch)?;
            }

            println!();

            // Convert all bytes to string (lossy to handle any remaining incomplete sequences)
            let response = String::from_utf8_lossy(&response_bytes).to_string();
            let parsed_response: LLMResponse = serde_json::from_str(&response)?;
            Ok(parsed_response)
        }
        Err(err) => Err(anyhow::anyhow!(err)),
    }
}

pub async fn get_llm_decision(
    env: Arc<Env>,
    current_input: LLMInput,
    history: Vec<HistoryEntry>,
) -> UserAction {
    let llm_result = get_response_from_llm(
        env.llm.as_ref(),
        env.base_prompt.session_path(),
        &current_input,
        &history,
    )
    .await;

    eprintln!("[DEBUG] llm_result: {:#?}", llm_result);

    match llm_result {
        Ok(llm_response) => UserAction::LLMDecisionResult(Ok(llm_response.outcome)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
