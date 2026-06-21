use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel},
    mtmd::{MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText},
    sampling::LlamaSampler,
};
use llama_cpp_2::{send_logs_to_tracing, LogOptions};
use std::{
    io::{self, Write},
    num::NonZero,
    sync::Arc,
};
use tokio::task::{spawn_blocking, JoinHandle};

use crate::{
    configuration::debug::DEBUG_LIVE_LLM_OUTPUT,
    model_pack::Pack,
    roles::{FormatFlags, PrimaryRole, Role, ThinkingPolicy},
};

const ADD_BOS_REEVAL_WHEN_CACHING_HITS: bool = false;
const DRY_BREAKS_LONG_STRINGS: bool = true;

pub struct PrimaryModel {
    pub mtmd: MtmdContext,
    pub model: Arc<LlamaModel>,
}

/// Inference knobs sourced from the pack manifest.
struct GenConfig {
    n_ctx: u32,
    n_batch: i32,
    batch_chunk: usize,
    max_generation_tokens: usize,
    top_k: i32,
    top_p: f32,
}

pub struct LlamaCppService {
    primary: Arc<PrimaryModel>,
    backend: Arc<LlamaBackend>,
    role: PrimaryRole,
    cfg: Arc<GenConfig>,
}

impl LlamaCppService {
    fn primary_model(
        backend: &LlamaBackend,
        model_params: &LlamaModelParams,
        pack: &Pack,
    ) -> anyhow::Result<LlamaModel> {
        let path = pack.model_path();
        println!("Loading primary model from: {}", path.display());
        let model = LlamaModel::load_from_file(backend, &path, model_params)?;
        println!("Loaded primary model from: {}", path.display());
        Ok(model)
    }

    fn mtmd_context(model: &LlamaModel, pack: &Pack) -> anyhow::Result<MtmdContext> {
        let path = pack.mmproj_path();
        println!("Loading multimodal projector from: {}", path.display());
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("mmproj path is not valid UTF-8: {}", path.display()))?;
        let mtmd = MtmdContext::init_from_file(path_str, model, &MtmdContextParams::default())?;
        println!(
            "Loaded multimodal projector from: {} (vision={}, audio={})",
            path.display(),
            mtmd.support_vision(),
            mtmd.support_audio()
        );
        Ok(mtmd)
    }

    pub async fn new() -> anyhow::Result<Self> {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));

        let pack = Pack::load()?;
        println!("Loaded model pack from: {}", pack.dir.display());

        let cfg = Arc::new(GenConfig {
            n_ctx: pack.manifest.context.n_ctx,
            n_batch: pack.manifest.context.n_batch,
            batch_chunk: pack.manifest.context.batch_chunk,
            max_generation_tokens: pack.manifest.context.max_generation_tokens,
            top_k: pack.manifest.sampling.top_k,
            top_p: pack.manifest.sampling.top_p,
        });
        let role = PrimaryRole::new(
            pack.template.clone(),
            pack.dir.clone(),
            FormatFlags {
                enable_thinking: pack.manifest.format.enable_thinking,
                add_generation_prompt: pack.manifest.format.add_generation_prompt,
            },
        );

        let backend = Arc::new(LlamaBackend::init()?);

        let primary_task: JoinHandle<anyhow::Result<Arc<PrimaryModel>>> = {
            let backend = Arc::clone(&backend);
            spawn_blocking(move || {
                let model_params = LlamaModelParams::default();
                let model = Arc::new(Self::primary_model(backend.as_ref(), &model_params, &pack)?);
                let mtmd = Self::mtmd_context(&model, &pack)?;
                Ok(Arc::new(PrimaryModel { mtmd, model }))
            })
        };
        let primary = primary_task.await??;

        Ok(Self { primary, backend, role, cfg })
    }

    pub fn role(&self) -> &dyn Role {
        &self.role
    }

    /// Run inference on an already-rendered prompt and return the raw generated text. Tokenizes,
    /// evaluates the prompt (text or multimodal), and samples until EOG or the token budget.
    pub async fn generate(
        &self,
        prompt: String,
        images: Vec<Arc<Vec<u8>>>,
        temperature: f32,
    ) -> anyhow::Result<String> {
        let primary = Arc::clone(&self.primary);
        let backend = Arc::clone(&self.backend);
        let cfg = Arc::clone(&self.cfg);
        let thinking = self.role.thinking();
        spawn_blocking(move || {
            if images.is_empty() {
                run_generation_text(&primary, &backend, &cfg, &prompt, temperature, thinking)
            } else {
                run_generation_mtmd(&primary, &backend, &cfg, &prompt, &images, temperature, thinking)
            }
        })
        .await?
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
    primary: &PrimaryModel,
    backend: &LlamaBackend,
    cfg: &GenConfig,
    prompt: &str,
    temperature: f32,
    thinking: ThinkingPolicy,
) -> anyhow::Result<String> {
    let model = primary.model.as_ref();
    let mut ctx = model.new_context(backend, context_params(cfg))?;

    let mut tokens = model.str_to_token(
        prompt,
        if ADD_BOS_REEVAL_WHEN_CACHING_HITS { AddBos::Always } else { AddBos::Never },
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
    generate(model, &mut ctx, tokens.len() as i32, temperature, cfg, thinking)
}

fn run_generation_mtmd(
    primary: &PrimaryModel,
    backend: &LlamaBackend,
    cfg: &GenConfig,
    prompt: &str,
    images: &[Arc<Vec<u8>>],
    temperature: f32,
    thinking: ThinkingPolicy,
) -> anyhow::Result<String> {
    let model = primary.model.as_ref();
    let mtmd = &primary.mtmd;

    let bitmaps = images
        .iter()
        .map(|bytes| MtmdBitmap::from_buffer(mtmd, bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let bitmap_refs: Vec<&MtmdBitmap> = bitmaps.iter().collect();

    let mut ctx = model.new_context(backend, context_params(cfg))?;

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
    generate(model, &mut ctx, n_past, temperature, cfg, thinking)
}

fn generate(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    mut n_cur: i32,
    temperature: f32,
    cfg: &GenConfig,
    thinking: ThinkingPolicy,
) -> anyhow::Result<String> {
    let mut samplers: Vec<LlamaSampler> = Vec::new();
    if !DRY_BREAKS_LONG_STRINGS {
        samplers.push(LlamaSampler::dry(model, 0.8, 1.75, 2, -1, ["\n", ":", "\"", "*"]));
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
            for forced in model.str_to_token(thinking.force_close, AddBos::Never)? {
                emit_token!(forced);
            }
            thinking_closed = true;
        }

        let token = sampler.sample(ctx, -1);
        if model.is_eog_token(token) {
            break;
        }
        emit_token!(token);

        if !thinking_closed && out_bytes.windows(close_marker.len()).any(|w| w == close_marker) {
            thinking_closed = true;
        }
    }

    Ok(String::from_utf8_lossy(&out_bytes).into_owned())
}
