use std::path::PathBuf;
use std::sync::Arc;

use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::mtmd::mtmd_default_marker;
use tokio::task::spawn_blocking;

use super::local_model::{self, LocalModel};
use super::{ParsedResponse, RenderInputs, Role, ThinkingPolicy};
use crate::llm::native::chat_format::ChatMessage;
use crate::llm::native::model_pack::Pack;
use crate::prompts::{PRIMARY_SYSTEM_PROMPT, PRIMARY_TEMPERATURE};

const THINKING_NUDGE: &str =
    "Wait — I'm going in circles. I'll stop thinking and act now: either answer the user, or make a tool call if that's what's needed.";
const MAX_THINKING_TOKENS: usize = 2000;

const PACK_DIR_ENV: &str = "MODEL_PACK_DIR";
const DEFAULT_PACK_DIR: &str = "./models/qwen3-8-27b";

fn pack_dir() -> PathBuf {
    std::env::var_os(PACK_DIR_ENV).map_or_else(|| DEFAULT_PACK_DIR.into(), PathBuf::from)
}

pub struct PrimaryRole {
    model: LocalModel,
}

impl PrimaryRole {
    pub fn load(backend: Arc<LlamaBackend>) -> anyhow::Result<Self> {
        let pack = Pack::load_from(&pack_dir())?;
        Ok(PrimaryRole { model: local_model::load_model(backend, &pack)? })
    }

    pub fn bench(&self, user_message: &str, image: Option<Vec<u8>>) -> anyhow::Result<()> {
        let content = match image {
            Some(_) => format!("{user_message}\n{}", mtmd_default_marker()),
            None => user_message.to_string(),
        };
        let images: Vec<Arc<Vec<u8>>> = image.into_iter().map(Arc::new).collect();
        let messages = [ChatMessage::user(content)];
        let prompt = self.render_prompt(&RenderInputs {
            system: Some(PRIMARY_SYSTEM_PROMPT.as_str()),
            messages: &messages,
            tools: None,
            footer: None,
        })?;
        local_model::bench(&self.model, &prompt, &images, PRIMARY_TEMPERATURE, &self.thinking())
    }
}

impl Role for PrimaryRole {
    fn model_path(&self) -> PathBuf {
        pack_dir()
    }

    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String> {
        self.model.render(inputs)
    }

    async fn generate(
        self: Arc<Self>,
        prompt: String,
        images: Vec<Arc<Vec<u8>>>,
        temperature: f32,
    ) -> anyhow::Result<String> {
        let thinking = self.thinking();
        spawn_blocking(move || {
            local_model::run(&self.model, &prompt, &images, temperature, &thinking)
        })
        .await?
    }

    fn parse_response(&self, raw: &str) -> ParsedResponse {
        self.model.parse(raw)
    }

    fn thinking(&self) -> ThinkingPolicy {
        let close_marker = self.model.close_marker();
        ThinkingPolicy {
            force_close: format!("\n\n{THINKING_NUDGE}\n{close_marker}\n\n"),
            close_marker: close_marker.to_string(),
            max_tokens: MAX_THINKING_TOKENS,
        }
    }
}
