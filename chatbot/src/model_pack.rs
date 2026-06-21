use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A model pack: a self-contained folder holding everything model-specific — the GGUF weights, the
/// multimodal projector, our authored chat template, and a manifest of the knobs. Swap the folder
/// (via `MODEL_PACK_DIR`) and restart to change models; nothing model-specific is compiled in.
pub struct Pack {
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// Contents of the manifest's `template` file, read once at load.
    pub template: String,
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub model: String,
    pub mmproj: String,
    pub template: String,
    pub sampling: Sampling,
    pub context: Context,
    pub format: Format,
    pub thinking: Thinking,
}

#[derive(Debug, Deserialize)]
pub struct Sampling {
    pub top_k: i32,
    pub top_p: f32,
}

#[derive(Debug, Deserialize)]
pub struct Context {
    pub n_ctx: u32,
    pub n_batch: i32,
    pub batch_chunk: usize,
    pub max_generation_tokens: usize,
}

#[derive(Debug, Deserialize)]
pub struct Format {
    pub enable_thinking: bool,
    pub add_generation_prompt: bool,
}

#[derive(Debug, Deserialize)]
pub struct Thinking {
    /// The model's reasoning close marker, e.g. `</think>` (varies model to model).
    pub close_marker: String,
}

const DEFAULT_PACK_DIR: &str = "./models/qwen-qwen3-6-35b-a3b";

impl Pack {
    /// Load the pack named by `MODEL_PACK_DIR` (default: the bundled Qwen3.6 pack).
    pub fn load() -> anyhow::Result<Self> {
        let dir = PathBuf::from(
            std::env::var("MODEL_PACK_DIR").unwrap_or_else(|_| DEFAULT_PACK_DIR.to_string()),
        );
        Self::load_from(&dir)
    }

    pub fn load_from(dir: &Path) -> anyhow::Result<Self> {
        let manifest_path = dir.join("manifest.toml");
        let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|e| {
            anyhow::anyhow!("model pack: cannot read {}: {e}", manifest_path.display())
        })?;
        let manifest: Manifest = toml::from_str(&manifest_text)
            .map_err(|e| anyhow::anyhow!("model pack: invalid manifest.toml: {e}"))?;

        let template_path = dir.join(&manifest.template);
        let template = std::fs::read_to_string(&template_path).map_err(|e| {
            anyhow::anyhow!("model pack: cannot read template {}: {e}", template_path.display())
        })?;

        Ok(Pack { dir: dir.to_path_buf(), manifest, template })
    }

    pub fn model_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.model)
    }

    pub fn mmproj_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.mmproj)
    }
}
