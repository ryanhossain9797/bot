mod primary_agent;

use std::{
    io::{self, Write},
    sync::Arc,
};

use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    model::{AddBos, LlamaModel},
    mtmd::{MtmdBitmap, MtmdInputText},
    openai::OpenAIChatTemplateParams,
    sampling::LlamaSampler,
};
pub use primary_agent::*;
use tokio::task::spawn_blocking;

use crate::{
    configuration::debug::DEBUG_LIVE_LLM_OUTPUT,
    services::llama_cpp::{LlamaCppService, PrimaryModel},
};

const MAX_THINKING_TOKENS: usize = 1000;

const ADD_BOS_REEVAL_WHEN_CACHING_HITS: bool = false;

const THINKING_FORCE_CLOSE: &str =
    "\n\nWait — I'm going in circles. I'll stop thinking and act now: either answer the user, or make a tool call if that's what's needed.\n</think>\n\n";

fn run_generation_text(
    model: &LlamaModel,
    backend: &LlamaBackend,
    ctx_params: LlamaContextParams,
    batch_chunk_size: usize,
    prompt: &str,
    temperature: f32,
) -> anyhow::Result<String> {
    let mut ctx = model.new_context(backend, ctx_params)?;

    let mut tokens = model.str_to_token(prompt, if ADD_BOS_REEVAL_WHEN_CACHING_HITS { AddBos::Always } else { AddBos::Never })?;
    let max_input = LlamaCppService::get_context_size() - LlamaCppService::get_max_generation_tokens();
    if tokens.len() > max_input {
        let head = (max_input / 4).min(2048);
        let tail = max_input - head;
        let dropped = tokens.len() - max_input;
        eprintln!(
            "[ctx] prompt {} tokens exceeds budget {max_input}; dropping {dropped} middle tokens",
            tokens.len()
        );
        let mut trimmed = tokens[..head].to_vec();
        trimmed.extend_from_slice(&tokens[tokens.len() - tail..]);
        tokens = trimmed;
    }
    let mut batch = LlamaCppService::new_batch();
    let last = tokens.len() - 1;
    for (i, t) in tokens.iter().enumerate() {
        batch.add(*t, i as i32, &[0], i == last)?;
        if batch.n_tokens() >= batch_chunk_size as i32 {
            ctx.decode(&mut batch)?;
            batch.clear();
        }
    }
    if batch.n_tokens() > 0 {
        ctx.decode(&mut batch)?;
    }

    log_prompt(prompt);
    generate(model, &mut ctx, tokens.len() as i32, temperature)
}

fn log_prompt(prompt: &str) {
    if DEBUG_LIVE_LLM_OUTPUT {
        print!("{prompt}\n<<< generation >>>\n");
        let _ = io::stdout().flush();
    }
}

fn run_generation_mtmd(
    primary: &PrimaryModel,
    backend: &LlamaBackend,
    ctx_params: LlamaContextParams,
    batch_chunk_size: usize,
    prompt: &str,
    images: &[Arc<Vec<u8>>],
    temperature: f32,
) -> anyhow::Result<String> {
    let model = primary.model.as_ref();
    let mtmd = &primary.mtmd;

    let bitmaps = images
        .iter()
        .map(|bytes| MtmdBitmap::from_buffer(mtmd, bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let bitmap_refs: Vec<&MtmdBitmap> = bitmaps.iter().collect();

    let mut ctx = model.new_context(backend, ctx_params)?;

    let chunks = mtmd.tokenize(
        MtmdInputText {
            text: prompt.to_string(),
            add_special: ADD_BOS_REEVAL_WHEN_CACHING_HITS,
            parse_special: true,
        },
        &bitmap_refs,
    )?;

    let n_past = chunks.eval_chunks(mtmd, &ctx, 0, 0, batch_chunk_size as i32, true)?;

    log_prompt(prompt);
    generate(model, &mut ctx, n_past, temperature)
}

fn generate(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    mut n_cur: i32,
    temperature: f32,
) -> anyhow::Result<String> {
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::dry(model, 0.8, 1.75, 2, -1, ["\n", ":", "\"", "*"]),
        LlamaSampler::temp(temperature),
        LlamaSampler::top_k(20),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::dist(0),
    ]);

    let mut out_bytes: Vec<u8> = Vec::new();
    let mut printed = 0usize;
    let max_generation_tokens = LlamaCppService::get_max_generation_tokens();
    let mut batch = LlamaCppService::new_batch();

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
        }};
    }

    for i in 0..max_generation_tokens {
        if !thinking_closed && i >= MAX_THINKING_TOKENS {
            for forced in model.str_to_token(THINKING_FORCE_CLOSE, AddBos::Never)? {
                emit_token!(forced);
            }
            thinking_closed = true;
        }

        let token = sampler.sample(ctx, -1);
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
    primary: Arc<PrimaryModel>,
    backend: Arc<LlamaBackend>,
    batch_chunk_size: usize,
    conversation: serde_json::Value,
    images: Vec<Arc<Vec<u8>>>,
    allow_tools: bool,
) -> anyhow::Result<serde_json::Value> {
    let model = primary.model.as_ref();
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

    let raw = if images.is_empty() {
        run_generation_text(
            model,
            backend.as_ref(),
            ctx_params,
            batch_chunk_size,
            &rendered.prompt,
            agent.temperature(),
        )?
    } else {
        run_generation_mtmd(
            primary.as_ref(),
            backend.as_ref(),
            ctx_params,
            batch_chunk_size,
            &rendered.prompt,
            &images,
            agent.temperature(),
        )?
    };

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
        primary: Arc<PrimaryModel>,
        backend: Arc<LlamaBackend>,
        batch_chunk_size: usize,
        conversation: serde_json::Value,
        images: Vec<Arc<Vec<u8>>>,
        allow_tools: bool,
    ) -> anyhow::Result<serde_json::Value> {
        let task = spawn_blocking(move || {
            respond_blocking(
                self,
                ctx_params,
                primary,
                backend,
                batch_chunk_size,
                conversation,
                images,
                allow_tools,
            )
        });

        task.await?
    }
}
