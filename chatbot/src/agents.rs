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

/// Safety cap on a single generation's reasoning: if the model hasn't closed its `<think>` block
/// within this many generated tokens, we force it shut (see [`THINKING_FORCE_CLOSE`]) so it commits
/// to an answer instead of looping. Generous on purpose — a net for runaway loops, not a routine
/// limiter (the overall cap is `LlamaCppService::get_max_generation_tokens`).
const MAX_THINKING_TOKENS: usize = 2048;

/// Injected verbatim (tokenized, then fed through the decode loop) to force-close a runaway
/// `<think>` block: a short first-person "stop and answer" stitch in the model's own voice, then the
/// closing tag. Reading as the model's own decision to wrap up yields a cleaner answer than a bare
/// `</think>`. It sits before `</think>`, so it lands in `reasoning_content` and the user never
/// sees it.
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

    // Accumulate RAW token bytes, not per-token `token_to_piece` strings: a multi-byte char (e.g.
    // an emoji) can straddle token boundaries as byte-fallback tokens, and the per-token+decoder
    // path drops the incomplete pieces (see #96). We decode the complete byte buffer at the end.
    let mut out_bytes: Vec<u8> = Vec::new();
    let mut printed = 0usize; // bytes of out_bytes already streamed to stdout under DEBUG
    let max_generation_tokens = LlamaCppService::get_max_generation_tokens();

    // Generation starts inside the <think> block (the template ends the prompt with
    // `<|im_start|>assistant\n<think>`), so track whether the model closes it. If it hasn't by
    // MAX_THINKING_TOKENS, force it shut to break reasoning loops.
    let mut thinking_closed = false;

    // Feed a token to the context, append its raw bytes, and stream any newly-completed UTF-8 to
    // stdout (DEBUG). Inline (macro, not closure) to avoid holding a borrow of ctx across the loop.
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
            // Inject the "stop and answer" stitch + </think>, then resume sampling so the model
            // produces the answer instead of looping.
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
    let tools_json = crate::types::user::ToolType::tools_json();
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
        // The model may emit multiple <tool_call> blocks in one turn; with this set to false,
        // parse_response_oaicompat hard-fails (`ffi error -3`) on such output (#89). We allow it and
        // run every call (#98) — the state machine fans them out and collects the results.
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
            "{}\n\nUse tools deliberately and answer once you've gathered enough. You can call multiple tools in one turn when that helps.\n\nIMPORTANT — A [Followup] message arrived while you were still replying or while tools were running, so the user hadn't seen the result yet (they never see tool calls or their outputs, only your replies). If it follows one of your replies: gauge what you already covered and build on it rather than repeat — or handle it normally if it's a different track. If it follows tool results: weigh it against those results and consider whether it needs new information before answering.\n\nCurrent date and time (UTC): {}",
            self.system_prompt,
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
