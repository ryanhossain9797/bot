use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, LlamaModel},
};
use llama_cpp_2::{send_logs_to_tracing, LogOptions};
use std::num::NonZero;

use crate::agents::{Agent, EXECUTOR_AGENT_IMPL, THINKING_AGENT_IMPL};

pub struct LlamaCppService {
    thinking_model: LlamaModel,
    executor_model: LlamaModel,
    backend: LlamaBackend,
    thinking_agent: Agent,
    executor_agent: Agent,
}

impl LlamaCppService {
    const CONTEXT_SIZE: NonZero<u32> = NonZero::<u32>::new(32768).unwrap();
    const BATCH_CHUNK_SIZE: usize = 2048;
    const MAX_GENERATION_TOKENS: usize = 8192;
    const TEMPERATURE: f32 = 0.25;

    pub const fn get_max_generation_tokens() -> usize {
        Self::MAX_GENERATION_TOKENS
    }

    fn thinking_model(
        backend: &LlamaBackend,
        model_params: &LlamaModelParams,
    ) -> anyhow::Result<LlamaModel> {
        let thinking_model_path = std::env::var("THINKING_MODEL_PATH")
            .unwrap_or_else(|_| "./models/GLM-4-32B-0414-Q8_0-matteov2.gguf".to_string());
        println!("Loading thinking model from: {}", thinking_model_path);

        Ok(LlamaModel::load_from_file(
            backend,
            &thinking_model_path,
            model_params,
        )?)
    }

    fn executor_model(
        backend: &LlamaBackend,
        model_params: &LlamaModelParams,
    ) -> anyhow::Result<LlamaModel> {
        let executor_model_path = std::env::var("EXECUTOR_MODEL_PATH")
            .unwrap_or_else(|_| "./models/qwen2.5-0.5b-instruct-q8_0.gguf".to_string());
        println!("Loading executor model from: {}", executor_model_path);
        Ok(LlamaModel::load_from_file(
            &backend,
            &executor_model_path,
            &model_params,
        )?)
    }

    pub fn new() -> anyhow::Result<Self> {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));

        let backend = LlamaBackend::init()?;
        let model_params = LlamaModelParams::default();

        let thinking_model = Self::thinking_model(&backend, &model_params)?;
        let executor_model = Self::executor_model(&backend, &model_params)?;

        let thinking_agent = THINKING_AGENT_IMPL;
        let executor_agent = EXECUTOR_AGENT_IMPL;

        println!("Creating session files");
        if let Err(e) = thinking_agent.create_session_file(
            &thinking_model,
            &backend,
            Self::context_params(),
            Self::new_batch(),
            Self::BATCH_CHUNK_SIZE,
        ) {
            eprintln!(
                "Warning: Failed to create session file for thinking agent: {}",
                e
            );
            eprintln!("The bot will continue without session file caching.");
        }

        if let Err(e) = executor_agent.create_session_file(
            &executor_model,
            &backend,
            Self::context_params(),
            Self::new_batch(),
            Self::BATCH_CHUNK_SIZE,
        ) {
            eprintln!(
                "Warning: Failed to create session file for executor agent: {}",
                e
            );
            eprintln!("The bot will continue without session file caching.");
        }

        Ok(Self {
            thinking_model,
            executor_model,
            backend,
            thinking_agent,
            executor_agent,
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
        self.thinking_agent.get_response(
            Self::context_params(),
            &self.thinking_model,
            &self.backend,
            Self::CONTEXT_SIZE.get(),
            Self::TEMPERATURE,
            Self::BATCH_CHUNK_SIZE,
            dynamic_prompt,
        )
    }

    pub fn get_executor_response(&self, dynamic_prompt: &str) -> anyhow::Result<String> {
        self.executor_agent.get_response(
            Self::context_params(),
            &self.executor_model,
            &self.backend,
            Self::CONTEXT_SIZE.get(),
            Self::TEMPERATURE,
            Self::BATCH_CHUNK_SIZE,
            dynamic_prompt,
        )
    }
}
