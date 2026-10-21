
use llama_cpp_2::{
    context::{
        params::{LlamaContextParams, LlamaContextType},
        LlamaContext,
    },
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel},
    mtmd::{MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText},
    sampling::LlamaSampler,
    speculative::{MtpSpeculative, MtpSpeculativeParams},
    token::LlamaToken,
};
use std::{
    io::{self, Write},
    num::NonZero,
    sync::Arc,
    time::Instant,
};

use super::parsers::{self, Parser};
use super::{FormatFlags, ParsedResponse, RenderInputs, ThinkingPolicy};
use crate::{configuration::debug::DEBUG_LIVE_LLM_OUTPUT, model_pack::Pack};

const ADD_BOS_REEVAL_WHEN_CACHING_HITS: bool = false;
const DRY_BREAKS_LONG_STRINGS: bool = true;

pub(super) struct LocalModel {
    mtmd: Option<MtmdContext>,
    model: LlamaModel,
    backend: Arc<LlamaBackend>,
    cfg: GenConfig,
    template: String,
    flags: FormatFlags,
    close_marker: String,
    parser: &'static dyn Parser,
    mtp: Option<MtpConfig>,
}

impl LocalModel {
    pub(super) fn render(
        &self,
        system_prompt: &str,
        inputs: &RenderInputs,
    ) -> anyhow::Result<String> {
        super::render::render(&self.template, system_prompt, inputs, self.flags)
    }

    pub(super) fn parse(&self, raw: &str) -> ParsedResponse {
        self.parser.parse(raw, &self.close_marker)
    }

    pub(super) fn close_marker(&self) -> &str {
        &self.close_marker
    }
}

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

#[derive(Clone, Copy)]
struct MtpConfig {
    draft_max: i32,
    p_min: f32,
}

/// MTP is chosen per model: on when the GGUF carries an MTP head and the pack doesn't opt out.
fn detect_mtp(model: &LlamaModel, pack: &Pack) -> Option<MtpConfig> {
    let n_nextn = model
        .meta_val_str("general.architecture")
        .ok()
        .and_then(|arch| model.meta_val_str(&format!("{arch}.nextn_predict_layers")).ok())
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let spec = &pack.manifest.speculative;

    if n_nextn == 0 {
        println!("[mtp] model has no MTP head — standard decoding");
        None
    } else if !spec.mtp {
        println!("[mtp] model has {n_nextn} MTP layer(s) but the pack disables it — standard decoding");
        None
    } else if spec.draft_max <= 0 {
        println!("[mtp] draft_max = {} — standard decoding", spec.draft_max);
        None
    } else {
        println!(
            "[mtp] enabled: {n_nextn} MTP layer(s), up to {} draft tokens per step",
            spec.draft_max
        );
        Some(MtpConfig {
            draft_max: spec.draft_max,
            p_min: spec.p_min,
        })
    }
}

pub(super) fn load_model(backend: Arc<LlamaBackend>, pack: &Pack) -> anyhow::Result<LocalModel> {
    let model_path = pack.model_path();
    println!("Loading model from: {}", model_path.display());
    let model = LlamaModel::load_from_file(&backend, &model_path, &LlamaModelParams::default())?;
    println!("Loaded model from: {}", model_path.display());

    let mtmd = match pack.mmproj_path() {
        None => {
            println!("No multimodal projector in pack — loading text-only");
            None
        }
        Some(mmproj_path) => {
            println!(
                "Loading multimodal projector from: {}",
                mmproj_path.display()
            );
            let mmproj_str = mmproj_path.to_str().ok_or_else(|| {
                anyhow::anyhow!("mmproj path is not valid UTF-8: {}", mmproj_path.display())
            })?;
            let mtmd =
                MtmdContext::init_from_file(mmproj_str, &model, &MtmdContextParams::default())?;
            println!(
                "Loaded multimodal projector from: {} (vision={}, audio={})",
                mmproj_path.display(),
                mtmd.support_vision(),
                mtmd.support_audio()
            );
            Some(mtmd)
        }
    };

    let mtp = detect_mtp(&model, pack);

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
        mtp,
    })
}

pub(super) fn run(
    model: &LocalModel,
    prompt: &str,
    images: &[Arc<Vec<u8>>],
    temperature: f32,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<String> {
    let cfg = &model.cfg;
    match (&model.mtmd, images.is_empty()) {
        (_, true) => match model.mtp {
            Some(mtp) => run_generation_text_mtp(model, mtp, cfg, prompt, temperature, thinking),
            None => run_generation_text(model, cfg, prompt, temperature, thinking),
        },
        // MTP drafting skips embedding batches, so image prompts take the standard path.
        (Some(mtmd), false) => {
            run_generation_mtmd(model, mtmd, cfg, prompt, images, temperature, thinking)
        }
        (None, false) => Err(anyhow::anyhow!(
            "text-only model received {} image(s) but has no multimodal projector",
            images.len()
        )),
    }
}

/// Dev-only comparison: the same prompt with standard decoding, then with MTP if the model has it.
pub(super) fn bench(
    model: &LocalModel,
    prompt: &str,
    temperature: f32,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<()> {
    let cfg = &model.cfg;
    let standard = run_generation_text(model, cfg, prompt, temperature, thinking)?;
    println!("[bench] standard output ({} chars): {}", standard.len(), preview(&standard));
    match model.mtp {
        Some(mtp) => {
            let speculative =
                run_generation_text_mtp(model, mtp, cfg, prompt, temperature, thinking)?;
            println!("[bench] mtp output ({} chars): {}", speculative.len(), preview(&speculative));
        }
        None => println!("[bench] model has no MTP head — nothing to compare"),
    }
    Ok(())
}

fn preview(text: &str) -> String {
    let tail: String = text.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
    tail.replace('\n', "⏎")
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

/// Tokenize the prompt, dropping middle tokens if it would leave less than `reserve` positions.
fn prompt_tokens(
    llama: &LlamaModel,
    cfg: &GenConfig,
    prompt: &str,
    reserve: usize,
) -> anyhow::Result<Vec<LlamaToken>> {
    let mut tokens = llama.str_to_token(
        prompt,
        if ADD_BOS_REEVAL_WHEN_CACHING_HITS {
            AddBos::Always
        } else {
            AddBos::Never
        },
    )?;
    let max_input = cfg.n_ctx as usize - reserve;
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
    Ok(tokens)
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

    let tokens = prompt_tokens(llama, cfg, prompt, cfg.max_generation_tokens)?;
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
    mtmd: &MtmdContext,
    cfg: &GenConfig,
    prompt: &str,
    images: &[Arc<Vec<u8>>],
    temperature: f32,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<String> {
    let llama = &model.model;

    let bitmaps = images
        .iter()
        .map(|bytes| MtmdBitmap::from_buffer(mtmd, bytes, false))
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

/// Target context with recurrent-state snapshots so a hybrid model can roll back rejected draft
/// tokens, plus a same-model MTP draft context.
fn new_mtp<'m>(model: &'m LocalModel, mtp: MtpConfig) -> anyhow::Result<MtpSpeculative<'m>> {
    let llama = &model.model;
    let target = llama.new_context(
        &model.backend,
        context_params(&model.cfg).with_n_rs_seq(mtp.draft_max as u32),
    )?;
    let draft = llama.new_context(
        &model.backend,
        context_params(&model.cfg).with_context_type(LlamaContextType::Mtp),
    )?;
    Ok(MtpSpeculative::new(
        target,
        draft,
        MtpSpeculativeParams {
            n_max: mtp.draft_max,
            n_min: 0,
            p_min: mtp.p_min,
        },
    )?)
}

fn run_generation_text_mtp(
    model: &LocalModel,
    mtp: MtpConfig,
    cfg: &GenConfig,
    prompt: &str,
    temperature: f32,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<String> {
    let mut spec = match new_mtp(model, mtp) {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("[mtp] setup failed: {e:#} — falling back to standard decoding");
            return run_generation_text(model, cfg, prompt, temperature, thinking);
        }
    };

    let llama = &model.model;
    // A verification batch can run draft_max + 1 positions past the generation budget.
    let reserve = cfg.max_generation_tokens + mtp.draft_max as usize + 1;
    let tokens = prompt_tokens(llama, cfg, prompt, reserve)?;

    let mut batch = new_batch(cfg);
    let last = tokens.len() - 1;
    for (i, t) in tokens.iter().enumerate() {
        batch.add(*t, i as i32, &[0], i == last)?;
        if batch.n_tokens() >= cfg.batch_chunk as i32 || i == last {
            spec.target_context_mut().decode(&mut batch)?;
            spec.process(&batch)?;
            batch.clear();
        }
    }
    spec.begin(&tokens)?;

    log_prompt(prompt);
    generate_mtp(llama, &mut spec, tokens, temperature, cfg, thinking)
}

fn sampler_chain(model: &LlamaModel, temperature: f32, cfg: &GenConfig) -> LlamaSampler {
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
    LlamaSampler::chain_simple(samplers)
}

/// Generated text plus the thinking-budget bookkeeping shared by both decode paths.
struct Output<'a> {
    model: &'a LlamaModel,
    bytes: Vec<u8>,
    printed: usize,
    close_marker: &'a [u8],
    thinking_closed: bool,
    sampled: usize,
    started: Instant,
}

impl<'a> Output<'a> {
    fn new(model: &'a LlamaModel, thinking: &'a ThinkingPolicy) -> Self {
        Output {
            model,
            bytes: Vec::new(),
            printed: 0,
            close_marker: thinking.close_marker.as_bytes(),
            thinking_closed: false,
            sampled: 0,
            started: Instant::now(),
        }
    }

    fn needs_forced_close(&self, thinking: &ThinkingPolicy) -> bool {
        !self.thinking_closed && self.sampled >= thinking.max_tokens
    }

    /// A token the sampler chose; counts toward the generation and thinking budgets.
    fn push_sampled(&mut self, tok: LlamaToken) -> anyhow::Result<()> {
        let before = self.bytes.len();
        self.push(tok)?;
        self.sampled += 1;
        if !self.thinking_closed {
            let from = before.saturating_sub(self.close_marker.len());
            if self.bytes[from..]
                .windows(self.close_marker.len())
                .any(|w| w == self.close_marker)
            {
                self.thinking_closed = true;
            }
        }
        Ok(())
    }

    /// A token injected by the thinking budget.
    fn push_forced(&mut self, tok: LlamaToken) -> anyhow::Result<()> {
        self.push(tok)
    }

    fn push(&mut self, tok: LlamaToken) -> anyhow::Result<()> {
        self.bytes
            .extend_from_slice(&self.model.token_to_piece_bytes(tok, 512, true, None)?);
        if DEBUG_LIVE_LLM_OUTPUT {
            let valid = match std::str::from_utf8(&self.bytes) {
                Ok(s) => s.len(),
                Err(e) => e.valid_up_to(),
            };
            if valid > self.printed {
                print!("{}", String::from_utf8_lossy(&self.bytes[self.printed..valid]));
                let _ = io::stdout().flush();
                self.printed = valid;
            }
        }
        Ok(())
    }

    fn tokens_per_second(&self) -> f64 {
        self.sampled as f64 / self.started.elapsed().as_secs_f64().max(f64::EPSILON)
    }

    fn finish(self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

fn decode_one(
    ctx: &mut LlamaContext,
    batch: &mut LlamaBatch,
    tok: LlamaToken,
    n_cur: &mut i32,
) -> anyhow::Result<()> {
    batch.clear();
    batch.add(tok, *n_cur, &[0], true)?;
    ctx.decode(batch)?;
    *n_cur += 1;
    Ok(())
}

fn generate(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    mut n_cur: i32,
    temperature: f32,
    cfg: &GenConfig,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<String> {
    let mut sampler = sampler_chain(model, temperature, cfg);
    let mut out = Output::new(model, thinking);
    let mut batch = new_batch(cfg);

    while out.sampled < cfg.max_generation_tokens {
        if out.needs_forced_close(thinking) {
            for forced in model.str_to_token(&thinking.force_close, AddBos::Never)? {
                out.push_forced(forced)?;
                decode_one(ctx, &mut batch, forced, &mut n_cur)?;
            }
            out.thinking_closed = true;
        }

        let token = sampler.sample(ctx, -1);
        if model.is_eog_token(token) {
            break;
        }
        out.push_sampled(token)?;
        decode_one(ctx, &mut batch, token, &mut n_cur)?;
    }

    println!(
        "[gen] {} tokens at {:.1} tok/s",
        out.sampled,
        out.tokens_per_second()
    );
    Ok(out.finish())
}

/// Speculative loop: `pending` is the latest sampled token — already in the output, not yet in
/// the target KV. Each step drafts after it, verifies `[pending, draft..]` in one target batch,
/// keeps the longest prefix the sampler agrees with plus the sampler's own next token, and rolls
/// both contexts back past the rejected drafts.
fn generate_mtp(
    model: &LlamaModel,
    spec: &mut MtpSpeculative,
    mut history: Vec<LlamaToken>,
    temperature: f32,
    cfg: &GenConfig,
    thinking: &ThinkingPolicy,
) -> anyhow::Result<String> {
    let mut sampler = sampler_chain(model, temperature, cfg);
    let mut out = Output::new(model, thinking);
    let mut batch = new_batch(cfg);
    let mut n_past = history.len() as i32;
    let (mut drafted, mut accepted) = (0usize, 0usize);

    let mut pending = sampler.sample(spec.target_context(), -1);
    if model.is_eog_token(pending) {
        return Ok(out.finish());
    }
    out.push_sampled(pending)?;

    'generation: while out.sampled < cfg.max_generation_tokens {
        if out.needs_forced_close(thinking) {
            let forced = model.str_to_token(&thinking.force_close, AddBos::Never)?;
            batch.clear();
            batch.add(pending, n_past, &[0], forced.is_empty())?;
            for (k, &tok) in forced.iter().enumerate() {
                batch.add(tok, n_past + 1 + k as i32, &[0], k + 1 == forced.len())?;
                out.push_forced(tok)?;
            }
            spec.target_context_mut().decode(&mut batch)?;
            spec.process(&batch)?;
            history.push(pending);
            history.extend_from_slice(&forced);
            n_past += batch.n_tokens();
            out.thinking_closed = true;

            pending = sampler.sample(spec.target_context(), batch.n_tokens() - 1);
            if model.is_eog_token(pending) {
                break;
            }
            out.push_sampled(pending)?;
            continue;
        }

        let draft = spec.draft(n_past, pending, &history)?;
        // Drafting wrote [pending, draft..] into the draft KV; process() below re-decodes those
        // positions from the verified batch, so clear them first.
        spec.draft_context_mut()
            .kv_cache_seq_rm(0, Some(n_past as u32), None)?;

        batch.clear();
        batch.add(pending, n_past, &[0], true)?;
        for (k, &tok) in draft.iter().enumerate() {
            batch.add(tok, n_past + 1 + k as i32, &[0], true)?;
        }
        spec.target_context_mut().decode(&mut batch)?;
        spec.process(&batch)?;

        let mut ids = Vec::with_capacity(draft.len() + 1);
        for k in 0..=draft.len() {
            let tok = sampler.sample(spec.target_context(), k as i32);
            ids.push(tok);
            if k == draft.len() || tok != draft[k] {
                break;
            }
        }
        let n_accepted = ids.len() - 1;
        spec.accept(n_accepted as u16)?;
        drafted += draft.len();
        accepted += n_accepted;

        history.push(pending);
        history.extend_from_slice(&ids[..n_accepted]);
        n_past += 1 + n_accepted as i32;
        spec.target_context_mut()
            .kv_cache_seq_rm(0, Some(n_past as u32), None)?;
        spec.draft_context_mut()
            .kv_cache_seq_rm(0, Some(n_past as u32), None)?;

        for &tok in &ids {
            if model.is_eog_token(tok) {
                break 'generation;
            }
            out.push_sampled(tok)?;
            if out.sampled >= cfg.max_generation_tokens {
                break 'generation;
            }
        }
        pending = ids[n_accepted];
    }

    let rate = if drafted == 0 {
        0.0
    } else {
        100.0 * accepted as f64 / drafted as f64
    };
    println!(
        "[gen] {} tokens at {:.1} tok/s (mtp: accepted {accepted}/{drafted} drafts, {rate:.0}%)",
        out.sampled,
        out.tokens_per_second()
    );
    Ok(out.finish())
}
