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

    // Qwen3 sampler: temp / top_k 20 / top_p 0.95, no repeat penalty.
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
    allow_tools: bool,
) -> anyhow::Result<serde_json::Value> {
    // Prepend the system turn (persona + current time), then render with the model's template. At
    // the budget cap, advertise no tools so the model physically cannot emit another tool call (the
    // synthesis nudge itself rides the message stream, not this stable system turn).
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
        tools_json: if allow_tools {
            Some(tools_json.as_str())
        } else {
            None
        },
        tool_choice: None,
        json_schema: None,
        grammar: None,
        reasoning_format: Some("auto"),
        chat_template_kwargs: None,
        add_generation_prompt: true,
        use_jinja: true,
        // The model may emit multiple <tool_call> blocks in one turn regardless; with this set to
        // false, parse_response_oaicompat hard-fails (`ffi error -3`) on such output and the turn
        // dies silently (see #89). Allow it so parsing succeeds; we still run only the first call
        // (first_tool_call warns on extras) until multi-tool execution lands.
        parallel_tool_calls: true,
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

    let parsed = rendered.parse_response_oaicompat(&raw, false)?;
    Ok(serde_json::from_str(&parsed)?)
}

/// A conversational agent: a persona (system prompt) plus its sampling temperature.
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

    pub fn system_content(&self) -> String {
        format!(
            "{}\n\nYou have up to {} tool calls per turn. Use them deliberately and prefer answering once you have gathered enough to respond.\n\nCurrent date and time (UTC): {}",
            self.system_prompt,
            crate::models::user::MAX_TOOL_ROUNDS,
            Utc::now().format("%Y-%m-%d %H:%M")
        )
    }

    pub async fn respond(
        &'static self,
        ctx_params: LlamaContextParams,
        model: Arc<LlamaModel>,
        backend: Arc<LlamaBackend>,
        batch_chunk_size: usize,
        conversation: serde_json::Value,
        allow_tools: bool,
    ) -> anyhow::Result<serde_json::Value> {
        let task = spawn_blocking(move || {
            respond_blocking(
                self,
                ctx_params,
                model,
                backend,
                batch_chunk_size,
                conversation,
                allow_tools,
            )
        });

        task.await?
    }
}
