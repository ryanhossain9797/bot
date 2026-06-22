//! The loaded model and the inference mechanics around it. `LocalModel` is everything a model
//! folder defines, loaded/resolved: weights + mmproj + backend handle, the sampling/context config,
//! and the wire-format facts (template, flags, reasoning marker, and the parser it names). It owns
//! `render` and `parse` because those *are* the model's wire format; the free functions below are the
//! generic decode loop. The role layers identity (system prompt, temperature) on top — it never
//! holds format facts, since those belong to the model, not the role.
//!
//! The `LlamaBackend` is a process singleton, created once in `init_env` and `Arc`-cloned into each
//! `LocalModel` — so the model is self-contained for inference and the `Role` contract never has to
//! mention it (a remote model would carry no backend at all).

use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel},
    mtmd::{MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText},
    sampling::LlamaSampler,
};
use std::{
    io::{self, Write},
    num::NonZero,
    sync::Arc,
};

use super::parsers::{self, Parser};
use super::{FormatFlags, ParsedResponse, RenderInputs, ThinkingPolicy};
use crate::{configuration::debug::DEBUG_LIVE_LLM_OUTPUT, model_pack::Pack};

const ADD_BOS_REEVAL_WHEN_CACHING_HITS: bool = false;
const DRY_BREAKS_LONG_STRINGS: bool = true;

/// A fully-loaded model: everything its folder defines. The runtime handles (weights, projector,
/// shared backend), the sampling/context config, and the wire-format facts (template, render flags,
/// reasoning marker, and the parser the manifest names). Self-contained — it can render, run, and
/// parse on its own.
pub(super) struct LocalModel {
    mtmd: MtmdContext,
    model: LlamaModel,
    backend: Arc<LlamaBackend>,
    cfg: GenConfig,
    template: String,
    flags: FormatFlags,
    close_marker: String,
    parser: &'static dyn Parser,
}

impl LocalModel {
    /// Render the final prompt in this model's wire format from a system prompt + conversation.
    pub(super) fn render(
        &self,
        system_prompt: &str,
        inputs: &RenderInputs,
    ) -> anyhow::Result<String> {
        super::render::render(&self.template, system_prompt, inputs, self.flags)
    }

    /// Decode raw generation into reasoning / content / tool calls using this model's parser.
    pub(super) fn parse(&self, raw: &str) -> ParsedResponse {
        self.parser.parse(raw, &self.close_marker)
    }

    /// The model's reasoning close marker — the role needs it to compose its thinking policy.
    pub(super) fn close_marker(&self) -> &str {
        &self.close_marker
    }
}

/// Inference knobs sourced from a pack manifest.
pub(super) struct GenConfig {
    pub n_ctx: u32,
    pub n_batch: i32,
    pub batch_chunk: usize,
    pub max_generation_tokens: usize,
    pub top_k: i32,
    pub top_p: f32,
}

impl GenConfig {
    pub(super) fn from_pack(pack: &Pack) -> Self {
        GenConfig {
            n_ctx: pack.manifest.context.n_ctx,
            n_batch: pack.manifest.context.n_batch,
            batch_chunk: pack.manifest.context.batch_chunk,
            max_generation_tokens: pack.manifest.context.max_generation_tokens,
            top_k: pack.manifest.sampling.top_k,
            top_p: pack.manifest.sampling.top_p,
        }
    }
}

/// Load a pack's weights and multimodal projector into memory, taking an `Arc` to the shared backend
/// to store alongside them. The loaded `LocalModel` then needs nothing external to run.
pub(super) fn load_model(backend: Arc<LlamaBackend>, pack: &Pack) -> anyhow::Result<LocalModel> {
    let model_path = pack.model_path();
    println!("Loading model from: {}", model_path.display());
    let model = LlamaModel::load_from_file(&backend, &model_path, &LlamaModelParams::default())?;
    println!("Loaded model from: {}", model_path.display());

    let mmproj_path = pack.mmproj_path();
    println!(
        "Loading multimodal projector from: {}",
        mmproj_path.display()
    );
    let mmproj_str = mmproj_path.to_str().ok_or_else(|| {
        anyhow::anyhow!("mmproj path is not valid UTF-8: {}", mmproj_path.display())
    })?;
    let mtmd = MtmdContext::init_from_file(mmproj_str, &model, &MtmdContextParams::default())?;
    println!(
        "Loaded multimodal projector from: {} (vision={}, audio={})",
        mmproj_path.display(),
        mtmd.support_vision(),
        mtmd.support_audio()
    );

    Ok(LocalModel {
        mtmd,
        model,
        backend,
        cfg: GenConfig::from_pack(pack),
        template: pack.template.clone(),
        flags: FormatFlags {
            enable_thinking: pack.manifest.format.enable_thinking,
            add_generation_prompt: pack.manifest.format.add_generation_prompt,
        },
        close_marker: pack.manifest.thinking.close_marker.clone(),
        parser: parsers::from_name(&pack.manifest.format.parser)?,
    })
}

/// Run inference on an already-rendered prompt and return the raw generated text. Picks the text or
/// multimodal path by whether any images were supplied. Blocking; callers run it on a blocking task.
pub(super) fn run(
    model: &LocalModel,
    prompt: &str,
    images: &[Arc<Vec<u8>>],
    temperature: f32,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<String> {
    let cfg = &model.cfg;
    if images.is_empty() {
        run_generation_text(model, cfg, prompt, temperature, thinking)
    } else {
        run_generation_mtmd(model, cfg, prompt, images, temperature, thinking)
    }
}

fn context_params(cfg: &GenConfig) -> LlamaContextParams {
    LlamaContextParams::default()
        .with_n_ctx(NonZero::new(cfg.n_ctx))
        .with_n_batch(cfg.n_batch as u32)
        .with_n_threads(num_cpus::get() as i32)
        .with_n_threads_batch(num_cpus::get() as i32)
}

fn new_batch(cfg: &GenConfig) -> LlamaBatch<'static> {
    LlamaBatch::new(cfg.n_ctx as usize, 1)
}

fn log_prompt(prompt: &str) {
    if DEBUG_LIVE_LLM_OUTPUT {
        print!("{prompt}\n<<< generation >>>\n");
        let _ = io::stdout().flush();
    }
}

fn run_generation_text(
    model: &LocalModel,
    cfg: &GenConfig,
    prompt: &str,
    temperature: f32,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<String> {
    let llama = &model.model;
    let mut ctx = llama.new_context(&model.backend, context_params(cfg))?;

    let mut tokens = llama.str_to_token(
        prompt,
        if ADD_BOS_REEVAL_WHEN_CACHING_HITS {
            AddBos::Always
        } else {
            AddBos::Never
        },
    )?;
    let max_input = cfg.n_ctx as usize - cfg.max_generation_tokens;
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
    let mut batch = new_batch(cfg);
    let last = tokens.len() - 1;
    for (i, t) in tokens.iter().enumerate() {
        batch.add(*t, i as i32, &[0], i == last)?;
        if batch.n_tokens() >= cfg.batch_chunk as i32 {
            ctx.decode(&mut batch)?;
            batch.clear();
        }
    }
    if batch.n_tokens() > 0 {
        ctx.decode(&mut batch)?;
    }

    log_prompt(prompt);
    generate(
        llama,
        &mut ctx,
        tokens.len() as i32,
        temperature,
        cfg,
        thinking,
    )
}

fn run_generation_mtmd(
    model: &LocalModel,
    cfg: &GenConfig,
    prompt: &str,
    images: &[Arc<Vec<u8>>],
    temperature: f32,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<String> {
    let llama = &model.model;
    let mtmd = &model.mtmd;

    let bitmaps = images
        .iter()
        .map(|bytes| MtmdBitmap::from_buffer(mtmd, bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let bitmap_refs: Vec<&MtmdBitmap> = bitmaps.iter().collect();

    let mut ctx = llama.new_context(&model.backend, context_params(cfg))?;

    let chunks = mtmd.tokenize(
        MtmdInputText {
            text: prompt.to_string(),
            add_special: ADD_BOS_REEVAL_WHEN_CACHING_HITS,
            parse_special: true,
        },
        &bitmap_refs,
    )?;

    let n_past = chunks.eval_chunks(mtmd, &ctx, 0, 0, cfg.batch_chunk as i32, true)?;

    log_prompt(prompt);
    generate(llama, &mut ctx, n_past, temperature, cfg, thinking)
}

fn generate(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    mut n_cur: i32,
    temperature: f32,
    cfg: &GenConfig,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<String> {
    let mut samplers: Vec<LlamaSampler> = Vec::new();
    if !DRY_BREAKS_LONG_STRINGS {
        samplers.push(LlamaSampler::dry(
            model,
            0.8,
            1.75,
            2,
            -1,
            ["\n", ":", "\"", "*"],
        ));
    }
    samplers.push(LlamaSampler::temp(temperature));
    samplers.push(LlamaSampler::top_k(cfg.top_k));
    samplers.push(LlamaSampler::top_p(cfg.top_p, 1));
    samplers.push(LlamaSampler::dist(0));
    let mut sampler = LlamaSampler::chain_simple(samplers);

    let mut out_bytes: Vec<u8> = Vec::new();
    let mut printed = 0usize;
    let mut batch = new_batch(cfg);

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

    let close_marker = thinking.close_marker.as_bytes();
    for i in 0..cfg.max_generation_tokens {
        if !thinking_closed && i >= thinking.max_tokens {
            for forced in model.str_to_token(&thinking.force_close, AddBos::Never)? {
                emit_token!(forced);
            }
            thinking_closed = true;
        }

        let token = sampler.sample(ctx, -1);
        if model.is_eog_token(token) {
            break;
        }
        emit_token!(token);

        if !thinking_closed
            && out_bytes
                .windows(close_marker.len())
                .any(|w| w == close_marker)
        {
            thinking_closed = true;
        }
    }

    Ok(String::from_utf8_lossy(&out_bytes).into_owned())
}
