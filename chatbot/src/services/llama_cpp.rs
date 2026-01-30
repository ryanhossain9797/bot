use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
    token::LlamaToken,
    TokenToStringError,
};
use llama_cpp_2::{send_logs_to_tracing, LogOptions};
use std::num::NonZero;

use crate::agents::{Agent, THINKING_BASE_PROMPT_IMPL};

pub struct LlamaCppService {
    model: LlamaModel,
    backend: LlamaBackend,
    pub thinking_base_prompt: Agent,
}

impl LlamaCppService {
    const CONTEXT_SIZE: NonZero<u32> = NonZero::<u32>::new(32768).unwrap();
    pub const BATCH_CHUNK_SIZE: usize = 2048;
    const MAX_GENERATION_TOKENS: usize = 8192;
    const TEMPERATURE: f32 = 0.25;

    pub const fn get_max_generation_tokens() -> usize {
        Self::MAX_GENERATION_TOKENS
    }

    pub fn new() -> anyhow::Result<Self> {
        let model_path = std::env::var("MODEL_PATH")
            .unwrap_or_else(|_| "./models/GLM-4-32B-0414-Q8_0-matteov2.gguf".to_string());

        println!("Loading model from: {}", model_path);

        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
        let backend = LlamaBackend::init()?;

        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, &model_path, &model_params)?;

        let thinking_base_prompt = THINKING_BASE_PROMPT_IMPL;

        // Create session file during construction
        if let Err(e) = Self::create_session_file(&model, &backend, thinking_base_prompt) {
            eprintln!("Warning: Failed to create session file: {}", e);
            eprintln!("The bot will continue without session file caching.");
        }

        Ok(Self {
            model,
            backend,
            thinking_base_prompt,
        })
    }

    pub fn context_params() -> LlamaContextParams {
        LlamaContextParams::default()
            .with_n_ctx(Some(Self::CONTEXT_SIZE))
            .with_n_batch(4096)
            .with_n_threads(num_cpus::get() as i32)
            .with_n_threads_batch(num_cpus::get() as i32)
    }

    pub fn load_thinking_base_prompt(&self, ctx: &mut LlamaContext<'_>) -> anyhow::Result<usize> {
        self.thinking_base_prompt
            .load(ctx, &self.model, Self::CONTEXT_SIZE.get())
    }

    pub fn append_prompt(
        &self,
        ctx: &mut LlamaContext<'_>,
        dynamic_prompt: &str,
        start_pos: usize,
    ) -> anyhow::Result<(usize, i32)> {
        self.thinking_base_prompt
            .append_prompt(ctx, &self.model, dynamic_prompt, start_pos)
    }

    pub fn new_context(&self) -> anyhow::Result<LlamaContext<'_>> {
        let ctx_params = Self::context_params();
        Ok(self.model.new_context(&self.backend, ctx_params)?)
    }

    pub fn is_eog_token(&self, token: LlamaToken) -> bool {
        self.model.is_eog_token(token)
    }

    pub fn token_to_str(
        &self,
        token: LlamaToken,
        special: Special,
    ) -> Result<String, TokenToStringError> {
        self.model.token_to_str(token, special)
    }

    pub fn create_sampler(&self, base_prompt: Agent) -> LlamaSampler {
        LlamaSampler::chain_simple([
            LlamaSampler::temp(Self::TEMPERATURE),
            LlamaSampler::grammar(&self.model, base_prompt.associated_grammar(), "root")
                .expect("Failed to load grammar - check GBNF syntax"),
            LlamaSampler::dist(0),
        ])
    }

    pub fn new_batch() -> LlamaBatch<'static> {
        LlamaBatch::new(Self::CONTEXT_SIZE.get() as usize, 1)
    }

    fn create_session_file(
        model: &LlamaModel,
        backend: &LlamaBackend,
        base_prompt: Agent,
    ) -> anyhow::Result<()> {
        base_prompt.create_session_file(
            model,
            backend,
            Self::context_params(),
            Self::new_batch(),
            Self::BATCH_CHUNK_SIZE,
        )
    }
}
