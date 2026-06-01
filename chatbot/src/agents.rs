mod primary_agent;

use std::{
    io::{self, Write},
    sync::Arc,
};

use chrono::Utc;
use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    model::{AddBos, LlamaModel},
    openai::OpenAIChatTemplateParams,
    sampling::LlamaSampler,
};
pub use primary_agent::*;
use tokio::task::spawn_blocking;

use crate::{configuration::debug::DEBUG_LIVE_LLM_OUTPUT, services::llama_cpp::LlamaCppService};

/// Tokenize `prompt`, feed it to a fresh context in batch-sized chunks, then sample greedily until
/// the end-of-generation token (the template's real EOS) or the generation cap. Returns the raw
/// generated text. Multibyte UTF-8 that spans two tokens is handled by the stateful decoder.
fn run_generation(
    model: &LlamaModel,
    backend: &LlamaBackend,
    ctx_params: LlamaContextParams,
    batch_chunk_size: usize,
    prompt: &str,
    temperature: f32,
) -> anyhow::Result<String> {
    let mut ctx = model.new_context(backend, ctx_params)?;

    // ChatML has no BOS — the prompt already starts at `<|im_start|>`.
    let tokens = model.str_to_token(prompt, AddBos::Never)?;

    let mut batch = LlamaCppService::new_batch();
    let last = tokens.len() - 1;
    let mut last_idx = 0;
    for (i, t) in tokens.iter().enumerate() {
        batch.add(*t, i as i32, &[0], i == last)?;
        if batch.n_tokens() >= batch_chunk_size as i32 {
            ctx.decode(&mut batch)?;
            last_idx = batch.n_tokens() - 1;
            batch.clear();
        }
    }
    if batch.n_tokens() > 0 {
        ctx.decode(&mut batch)?;
        last_idx = batch.n_tokens() - 1;
    }
    let mut n_cur = tokens.len() as i32;

    // Qwen3 recommended sampler: temp / top_k 20 / top_p 0.95 / no repeat penalty.
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(temperature),
        LlamaSampler::top_k(20),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::dist(0),
    ]);

    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut out = String::new();
    let max_generation_tokens = LlamaCppService::get_max_generation_tokens();

    for _ in 0..max_generation_tokens {
        let token = sampler.sample(&ctx, last_idx);
        if model.is_eog_token(token) {
            break;
        }
        let piece = model.token_to_piece(token, &mut decoder, true, None)?;
        if DEBUG_LIVE_LLM_OUTPUT {
            print!("{piece}");
            let _ = io::stdout().flush();
        }
        out.push_str(&piece);

        batch.clear();
        batch.add(token, n_cur, &[0], true)?;
        ctx.decode(&mut batch)?;
        n_cur += 1;
        last_idx = batch.n_tokens() - 1;
    }

    Ok(out)
}

fn respond_blocking(
    agent: &'static Agent,
    ctx_params: LlamaContextParams,
    model: Arc<LlamaModel>,
    backend: Arc<LlamaBackend>,
    batch_chunk_size: usize,
    conversation: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    // Prepend the system turn (persona + current time), then render the whole thing with the
    // model's own chat template. No tools yet — the model can only reply with prose.
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": agent.system_content(),
    })];
    if let Some(arr) = conversation.as_array() {
        messages.extend(arr.iter().cloned());
    }
    let messages_json = serde_json::Value::Array(messages).to_string();

    let template = model.chat_template(None)?;
    let tools_json = crate::models::user::ToolCall::tools_json();
    let params = OpenAIChatTemplateParams {
        messages_json: &messages_json,
        tools_json: Some(tools_json.as_str()),
        tool_choice: None,
        json_schema: None,
        grammar: None,
        reasoning_format: Some("auto"),
        chat_template_kwargs: None,
        add_generation_prompt: true,
        use_jinja: true,
        parallel_tool_calls: false,
        enable_thinking: true,
        add_bos: false,
        add_eos: false,
        parse_tool_calls: true,
    };
    let rendered = model.apply_chat_template_oaicompat(&template, &params)?;

    if DEBUG_LIVE_LLM_OUTPUT {
        print!("{}\n<<< generation >>>\n", rendered.prompt);
        let _ = io::stdout().flush();
    }

    let raw = run_generation(
        model.as_ref(),
        backend.as_ref(),
        ctx_params,
        batch_chunk_size,
        &rendered.prompt,
        agent.temperature(),
    )?;

    // The binding splits `<think>` into reasoning_content and parses any tool calls for us.
    let parsed = rendered.parse_response_oaicompat(&raw, false)?;
    Ok(serde_json::from_str(&parsed)?)
}

/// A single conversational agent: a persona (system prompt) plus its sampling temperature.
/// Generation is driven by the model's native chat template — no grammar, no session cache.
/// Tools are global (see `crate::tools`), not per-agent.
#[derive(Clone, Copy)]
pub struct Agent {
    system_prompt: &'static str,
    temperature: f32,
}
impl Agent {
    pub const fn new(system_prompt: &'static str, temperature: f32) -> Self {
        Self {
            system_prompt,
            temperature,
        }
    }

    pub fn temperature(&self) -> f32 {
        self.temperature
    }

    /// The system turn content: the static persona plus the current date/time.
    pub fn system_content(&self) -> String {
        format!(
            "{}\n\nCurrent date and time (UTC): {}",
            self.system_prompt,
            Utc::now().format("%Y-%m-%d %H:%M")
        )
    }

    /// Render `conversation` (an OpenAI-style messages JSON array, WITHOUT the system turn),
    /// generate a reply, and return the parsed OpenAI assistant message
    /// (`{role, content, reasoning_content, tool_calls?}`).
    pub async fn respond(
        &'static self,
        ctx_params: LlamaContextParams,
        model: Arc<LlamaModel>,
        backend: Arc<LlamaBackend>,
        batch_chunk_size: usize,
        conversation: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let task = spawn_blocking(move || {
            respond_blocking(self, ctx_params, model, backend, batch_chunk_size, conversation)
        });

        task.await?
    }
}
