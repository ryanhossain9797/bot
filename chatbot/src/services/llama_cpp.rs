use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, LlamaModel},
};
use llama_cpp_2::{send_logs_to_tracing, LogOptions};
use std::{num::NonZero, sync::Arc};
use tokio::task::{spawn_blocking, JoinHandle};

use crate::agents::{Agent, EXECUTOR_AGENT_IMPL, THINKING_AGENT_IMPL};

pub struct LlamaCppService {
    thinking_model: Arc<LlamaModel>,
    executor_model: Arc<LlamaModel>,
    backend: Arc<LlamaBackend>,
    thinking_agent: &'static Agent,
    executor_agent: &'static Agent,
}

impl LlamaCppService {
    const CONTEXT_SIZE: NonZero<u32> = NonZero::<u32>::new(32768).unwrap();
    const BATCH_CHUNK_SIZE: usize = 2048;
    const MAX_GENERATION_TOKENS: usize = 8192;

    pub const fn get_max_generation_tokens() -> usize {
        Self::MAX_GENERATION_TOKENS
    }

    fn thinking_model(
        backend: &LlamaBackend,
        model_params: &LlamaModelParams,
    ) -> anyhow::Result<LlamaModel> {
        let thinking_model_path = std::env::var("THINKING_MODEL_PATH")
            .unwrap_or_else(|_| "./models/helcyon_mercury_v3.0-Q6_K.gguf".to_string());
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
            .unwrap_or_else(|_| "./models/Qwen2.5-Coder-14B-Instruct-Q4_K_M.gguf".to_string());
        println!("Loading executor model from: {}", executor_model_path);
        Ok(LlamaModel::load_from_file(
            &backend,
            &executor_model_path,
            &model_params,
        )?)
    }

    pub async fn new() -> anyhow::Result<Self> {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));

        let backend = LlamaBackend::init()?;

        let thinking_agent = &THINKING_AGENT_IMPL;
        let executor_agent = &EXECUTOR_AGENT_IMPL;

        let backend_arc = Arc::new(backend);

        println!("Creating session files in parallel");

        // Spawn both session file creation tasks in parallel using spawn_blocking
        let thinking_task: JoinHandle<anyhow::Result<Arc<LlamaModel>>> = {
            let backend = Arc::clone(&backend_arc);
            spawn_blocking(move || {
                let model_params = LlamaModelParams::default();
                let model = Arc::new(Self::thinking_model(backend.as_ref(), &model_params)?);

                thinking_agent.create_session_file(
                    &model,
                    &backend,
                    Self::context_params(),
                    Self::new_batch(),
                    Self::BATCH_CHUNK_SIZE,
                )?;
                Ok(model)
            })
        };

        let executor_task: JoinHandle<anyhow::Result<Arc<LlamaModel>>> = {
            let backend = Arc::clone(&backend_arc);
            spawn_blocking(move || {
                let model_params = LlamaModelParams::default();
                let model = Arc::new(Self::executor_model(&backend, &model_params)?);
                executor_agent.create_session_file(
                    &model,
                    &backend,
                    Self::context_params(),
                    Self::new_batch(),
                    Self::BATCH_CHUNK_SIZE,
                )?;
                Ok(model)
            })
        };

        // Wait for both tasks to complete using try_join!
        let (thinking_model, executor_model) = tokio::try_join!(thinking_task, executor_task)?;

        Ok(Self {
            thinking_model: thinking_model?,
            executor_model: executor_model?,
            backend: backend_arc,
            thinking_agent: &thinking_agent,
            executor_agent: &executor_agent,
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

    pub async fn get_thinking_response(&self, dynamic_prompt: &str) -> anyhow::Result<String> {
        self.thinking_agent
            .get_response(
                Self::context_params(),
                Arc::clone(&self.thinking_model),
                Arc::clone(&self.backend),
                Self::CONTEXT_SIZE.get(),
                Self::BATCH_CHUNK_SIZE,
                dynamic_prompt,
            )
            .await
    }

    pub async fn get_executor_response(&self, dynamic_prompt: &str) -> anyhow::Result<String> {
        self.executor_agent
            .get_response(
                Self::context_params(),
                Arc::clone(&self.executor_model),
                Arc::clone(&self.backend),
                Self::CONTEXT_SIZE.get(),
                Self::BATCH_CHUNK_SIZE,
                dynamic_prompt,
            )
            .await
    }
}
