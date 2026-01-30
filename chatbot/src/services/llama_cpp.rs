use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
};
use llama_cpp_2::{send_logs_to_tracing, LogOptions};
use std::num::NonZero;

use crate::agents::{Agent, THINKING_AGENT_PROMPT_IMPL};

pub struct LlamaCppService {
    model: LlamaModel,
    backend: LlamaBackend,
    thinking_base_prompt: Agent,
}

impl LlamaCppService {
    const CONTEXT_SIZE: NonZero<u32> = NonZero::<u32>::new(32768).unwrap();
    const BATCH_CHUNK_SIZE: usize = 2048;
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

        let thinking_agent = THINKING_AGENT_PROMPT_IMPL;

        // Create session file during construction
        if let Err(e) = thinking_agent.create_session_file(
            &model,
            &backend,
            Self::context_params(),
            Self::new_batch(),
            Self::BATCH_CHUNK_SIZE,
        ) {
            eprintln!("Warning: Failed to create session file: {}", e);
            eprintln!("The bot will continue without session file caching.");
        }

        Ok(Self {
            model,
            backend,
            thinking_base_prompt: thinking_agent,
        })
    }

    pub fn context_params() -> LlamaContextParams {
        LlamaContextParams::default()
            .with_n_ctx(Some(Self::CONTEXT_SIZE))
            .with_n_batch(4096)
            .with_n_threads(num_cpus::get() as i32)
            .with_n_threads_batch(num_cpus::get() as i32)
    }

    pub fn new_batch() -> LlamaBatch<'static> {
        LlamaBatch::new(Self::CONTEXT_SIZE.get() as usize, 1)
    }

    fn create_thinking_agent_session_file(
        model: &LlamaModel,
        backend: &LlamaBackend,
        agent: Agent,
    ) -> anyhow::Result<()> {
        agent.create_session_file(
            model,
            backend,
            Self::context_params(),
            Self::new_batch(),
            Self::BATCH_CHUNK_SIZE,
        )
    }

    pub fn get_thinking_response(&self, dynamic_prompt: &str) -> anyhow::Result<String> {
        self.thinking_base_prompt.get_response(
            Self::context_params(),
            &self.model,
            &self.backend,
            Self::CONTEXT_SIZE.get(),
            Self::TEMPERATURE,
            Self::BATCH_CHUNK_SIZE,
            dynamic_prompt,
        )
    }
}
