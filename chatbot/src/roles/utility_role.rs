use std::path::PathBuf;
use std::sync::Arc;

use llama_cpp_2::llama_backend::LlamaBackend;
use tokio::task::spawn_blocking;

use super::local_model::{self, LocalModel};
use super::{ParsedResponse, RenderInputs, Role, ThinkingPolicy};
use crate::model_pack::Pack;

const SYSTEM_PROMPT: &str = "You are a summarization engine. You are given the older portion of a \
    conversation between a user and an assistant. Produce a compact summary that preserves \
    everything later turns would need: who the participants are, stable facts and preferences the \
    user stated, decisions and conclusions reached, tools used and what they returned, and any \
    open threads or unfinished requests. Drop small talk and redundant back-and-forth. Write it as \
    dense prose or terse bullet points, in the third person.\n\n\
    The transcript may mark that an image was attached, viewed, or produced (e.g. '[attached 1 \
    image(s) — not visible to you]'). You cannot see these images. When one appears, record in the \
    summary that an image was present at that point, and — based only on what the surrounding \
    conversation says about it — add a brief description of what it most likely showed, clearly \
    marked as an assumption (e.g. 'presumably a screenshot of ...'). If nothing in the conversation \
    hints at its content, just note that an image was shared without guessing.\n\n\
    Output only the summary itself, with no preamble, no headers, and no commentary.";

const TEMPERATURE: f32 = 0.3;

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
    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn temperature(&self) -> f32 {
        TEMPERATURE
    }

    fn model_path(&self) -> PathBuf {
        pack_dir()
    }

    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String> {
        self.model.render(self.system_prompt(), inputs)
    }

    async fn generate(
        self: Arc<Self>,
        prompt: String,
        images: Vec<Arc<Vec<u8>>>,
    ) -> anyhow::Result<String> {
        let thinking = self.thinking();
        let temperature = self.temperature();
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
