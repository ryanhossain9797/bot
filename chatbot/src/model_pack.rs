use std::path::{Path, PathBuf};

use serde::Deserialize;

pub struct Pack {
    pub dir: PathBuf,
    pub manifest: Manifest,
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
    pub parser: String,
}

#[derive(Debug, Deserialize)]
pub struct Thinking {
    pub close_marker: String,
}

impl Pack {
    pub fn load_from(dir: &Path) -> anyhow::Result<Self> {
        let manifest_path = dir.join("manifest.toml");
        let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|e| {
            anyhow::anyhow!("model pack: cannot read {}: {e}", manifest_path.display())
        })?;
        let manifest: Manifest = toml::from_str(&manifest_text)
            .map_err(|e| anyhow::anyhow!("model pack: invalid manifest.toml: {e}"))?;

        let template_path = dir.join(&manifest.template);
        let template = std::fs::read_to_string(&template_path).map_err(|e| {
            anyhow::anyhow!(
                "model pack: cannot read template {}: {e}",
                template_path.display()
            )
        })?;

        Ok(Pack {
            dir: dir.to_path_buf(),
            manifest,
            template,
        })
    }

    pub fn model_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.model)
    }

    pub fn mmproj_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.mmproj)
    }
}
