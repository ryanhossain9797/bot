use crate::{
    models::user::{HistoryEntry, LLMDecisionType, LLMInput, UserAction},
    services::llm::{get_context_params, BASE_PROMPT, CONTEXT_SIZE},
    Env,
};
use llama_cpp_2::{
    llama_backend::LlamaBackend,
    model::{LlamaModel, Special},
    sampling::LlamaSampler,
};
use serde::Deserialize;
use std::{io::Write, sync::Arc};

const TEMPERATURE: f32 = 0.25;

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
    llm: &(LlamaModel, LlamaBackend),
    current_input: &LLMInput,
    history: &[HistoryEntry],
) -> anyhow::Result<LLMResponse> {
    let (model, backend) = llm;

    let ctx_params = get_context_params();

    let mut ctx = model.new_context(backend, ctx_params)?;

    let dynamic_prompt = build_dynamic_prompt(current_input, history);

    let base_token_count = BASE_PROMPT.load_base_prompt(&mut ctx, model, CONTEXT_SIZE.get())?;

    let (mut n_cur, mut batch) =
        BASE_PROMPT.append_prompt(&mut ctx, model, &dynamic_prompt, base_token_count)?;

    let grammar = include_str!("../../grammars/response.gbnf");

    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(TEMPERATURE),
        LlamaSampler::grammar(model, grammar, "root")
            .expect("Failed to load grammar - check GBNF syntax"),
        LlamaSampler::dist(0),
    ]);

    let max_tokens = 2000;
    let mut generated_tokens = Vec::new();

    for _ in 0..max_tokens {
        let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);

        if model.is_eog_token(new_token) {
            break;
        }

        generated_tokens.push(new_token);

        batch.clear();
        batch.add(new_token, n_cur as i32, &[0], true)?;
        n_cur += 1;

        ctx.decode(&mut batch)?;
    }

    let mut response_bytes = Vec::new();
    for token in &generated_tokens {
        if let Ok(output) = model.token_to_str(*token, Special::Tokenize) {
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
