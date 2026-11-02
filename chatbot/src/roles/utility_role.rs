use std::path::PathBuf;
use std::sync::Arc;

use llama_cpp_2::llama_backend::LlamaBackend;
use tokio::task::spawn_blocking;

use super::local_model::{self, LocalModel};
use super::{ParsedResponse, RenderInputs, Role, ThinkingPolicy};
use crate::model_pack::Pack;

const PACK_DIR_ENV: &str = "UTILITY_MODEL_PACK_DIR";
const DEFAULT_PACK_DIR: &str = "./models/qwen3-4b";

fn pack_dir() -> PathBuf {
    std::env::var_os(PACK_DIR_ENV).map_or_else(|| DEFAULT_PACK_DIR.into(), PathBuf::from)
}

pub struct UtilityRole {
    model: LocalModel,
}

impl UtilityRole {
    pub fn load(backend: Arc<LlamaBackend>) -> anyhow::Result<Self> {
        let pack = Pack::load_from(&pack_dir())?;
        Ok(UtilityRole { model: local_model::load_model(backend, &pack)? })
    }
}

impl Role for UtilityRole {
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
            force_close: String::new(),
            close_marker: close_marker.to_string(),
            max_tokens: usize::MAX,
        }
    }
}
