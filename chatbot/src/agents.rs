mod primary_agent;

use std::{
    io::{self, Write},
    sync::Arc,
};

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

const MAX_THINKING_TOKENS: usize = 2048;

const THINKING_FORCE_CLOSE: &str =
    "\n\nWait — I'm going in circles. I have enough to answer the user now, so I'll stop thinking and respond.\n</think>\n\n";

fn run_generation(
    model: &LlamaModel,
    backend: &LlamaBackend,
    ctx_params: LlamaContextParams,
    batch_chunk_size: usize,
    prompt: &str,
    temperature: f32,
) -> anyhow::Result<String> {
    let mut ctx = model.new_context(backend, ctx_params)?;

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

    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(temperature),
        LlamaSampler::top_k(20),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::dist(0),
    ]);

    let mut out_bytes: Vec<u8> = Vec::new();
    let mut printed = 0usize;
    let max_generation_tokens = LlamaCppService::get_max_generation_tokens();

    let mut thinking_closed = false;

    macro_rules! emit_token {
        ($tok:expr) => {{
            let tok = $tok;
            out_bytes.extend_from_slice(&model.token_to_piece_bytes(tok, 512, true, None)?);
            if DEBUG_LIVE_LLM_OUTPUT {
                let valid = match std::str::from_utf8(&out_bytes) {
                    Ok(s) => s.len(),
                    Err(e) => e.valid_up_to(),
                };
                if valid > printed {
                    print!("{}", String::from_utf8_lossy(&out_bytes[printed..valid]));
                    let _ = io::stdout().flush();
                    printed = valid;
                }
            }
            batch.clear();
            batch.add(tok, n_cur, &[0], true)?;
            ctx.decode(&mut batch)?;
            n_cur += 1;
            last_idx = batch.n_tokens() - 1;
        }};
    }

    for i in 0..max_generation_tokens {
        if !thinking_closed && i >= MAX_THINKING_TOKENS {
            for forced in model.str_to_token(THINKING_FORCE_CLOSE, AddBos::Never)? {
                emit_token!(forced);
            }
            thinking_closed = true;
        }

        let token = sampler.sample(&ctx, last_idx);
        if model.is_eog_token(token) {
            break;
        }
        emit_token!(token);

        if !thinking_closed && out_bytes.windows(8).any(|w| w == b"</think>") {
            thinking_closed = true;
        }
    }

    Ok(String::from_utf8_lossy(&out_bytes).into_owned())
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
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": agent.system_content(),
    })];
    if let Some(arr) = conversation.as_array() {
        messages.extend(arr.iter().cloned());
    }
    let messages_json = serde_json::Value::Array(messages).to_string();

    let template = model.chat_template(None)?;
    let tools_json = crate::types::conversation::ToolType::tools_json();
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

            pub fn system_content(&self) -> &'static str {
        self.system_prompt
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
