use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, LlamaModel},
};
use llama_cpp_2::{send_logs_to_tracing, LogOptions};
use std::{num::NonZero, sync::Arc};
use tokio::task::{spawn_blocking, JoinHandle};

use crate::agents::{Agent, PRIMARY_AGENT_IMPL};

pub struct LlamaCppService {
    primary_model: Arc<LlamaModel>,
    backend: Arc<LlamaBackend>,
    primary_agent: &'static Agent,
}

impl LlamaCppService {
    const CONTEXT_SIZE: NonZero<u32> = NonZero::<u32>::new(32768).unwrap();
    const BATCH_CHUNK_SIZE: usize = 2048;
    const MAX_GENERATION_TOKENS: usize = 8192;

    pub const fn get_max_generation_tokens() -> usize {
        Self::MAX_GENERATION_TOKENS
    }

    fn primary_model(
        backend: &LlamaBackend,
        model_params: &LlamaModelParams,
    ) -> anyhow::Result<LlamaModel> {
        let primary_model_path = std::env::var("PRIMARY_MODEL_PATH")
            .unwrap_or_else(|_| "./models/Qwen3.6-27B-Q4_K_M.gguf".to_string());
        println!("Loading primary model from: {}", primary_model_path);

        let model = LlamaModel::load_from_file(backend, &primary_model_path, model_params)?;
        println!("Loaded primary model from: {}", primary_model_path);
        Ok(model)
    }

    pub async fn new() -> anyhow::Result<Self> {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));

        let backend = LlamaBackend::init()?;

        let primary_agent = &PRIMARY_AGENT_IMPL;

        let backend_arc = Arc::new(backend);

        // Load off the runtime thread; spawn_blocking so more models can load in parallel later.
        let primary_task: JoinHandle<anyhow::Result<Arc<LlamaModel>>> = {
            let backend = Arc::clone(&backend_arc);
            spawn_blocking(move || {
                let model_params = LlamaModelParams::default();
                let model = Arc::new(Self::primary_model(backend.as_ref(), &model_params)?);
                Ok(model)
            })
        };

        let primary_model = primary_task.await??;

        Ok(Self {
            primary_model,
            backend: backend_arc,
            primary_agent,
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

    pub async fn get_primary_response(
        &self,
        conversation: serde_json::Value,
        allow_tools: bool,
        is_group: bool,
        bot_identity: &str,
    ) -> anyhow::Result<serde_json::Value> {
        self.primary_agent
            .respond(
                Self::context_params(),
                Arc::clone(&self.primary_model),
                Arc::clone(&self.backend),
                Self::BATCH_CHUNK_SIZE,
                conversation,
                allow_tools,
                is_group,
                bot_identity.to_string(),
            )
            .await
    }
}
