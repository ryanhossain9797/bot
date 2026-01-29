mod executor_base_prompt;
mod thinking_base_prompt;

pub use executor_base_prompt::*;
use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel},
};
pub use thinking_base_prompt::*;

use crate::services::llama_cpp::LlamaCppService;

#[derive(Clone, Copy)]
pub struct BasePrompt {
    prompt: &'static str,
    session_path: &'static str,
    associated_grammar: &'static str,
}

impl BasePrompt {
    pub const fn new(
        prompt: &'static str,
        session_path: &'static str,
        associated_grammar: &'static str,
    ) -> Self {
        Self {
            prompt,
            session_path,
            associated_grammar,
        }
    }

    pub fn as_str(&self) -> &str {
        self.prompt
    }

    pub fn session_path(&self) -> &str {
        self.session_path
    }

    pub fn associated_grammar(&self) -> &str {
        self.associated_grammar
    }

    pub fn create_session_file(
        &self,
        model: &LlamaModel,
        backend: &LlamaBackend,
        ctx_params: LlamaContextParams,
        mut batch: LlamaBatch<'static>,
        batch_limit: usize,
    ) -> anyhow::Result<()> {
        self.delete_current_system_prompt_session()?;

        println!("Creating session file at: {}", self.session_path);

        let mut ctx = model.new_context(backend, ctx_params)?;

        let tokens = model.str_to_token(self.as_str(), AddBos::Always)?;
        println!("Tokenized base prompt: {} tokens", tokens.len());

        for (i, token) in tokens.iter().enumerate() {
            let is_last = i == tokens.len() - 1;
            batch.add(*token, i as i32, &[0], is_last)?;

            if batch.n_tokens() >= batch_limit as i32 {
                println!("Decoding batch chunk ({} tokens)...", batch.n_tokens());
                ctx.decode(&mut batch)?;
                batch.clear();
            }
        }

        if batch.n_tokens() > 0 {
            println!(
                "Decoding final batch chunk ({} tokens)...",
                batch.n_tokens()
            );
            ctx.decode(&mut batch)?;
        }

        println!("Saving session file...");
        ctx.save_session_file(self.session_path, &tokens)?;

        let metadata = std::fs::metadata(self.session_path)?;
        let file_size_bytes = metadata.len();
        let file_size_mb = file_size_bytes as f64 / (1024.0 * 1024.0);

        println!(
            "Session file created successfully: {} ({} bytes, {:.2} MB)",
            self.session_path, file_size_bytes, file_size_mb
        );

        Ok(())
    }

    pub fn load(
        &self,
        ctx: &mut LlamaContext<'_>,
        model: &LlamaModel,
        context_size: u32,
    ) -> anyhow::Result<usize> {
        let session_load_result = ctx.load_session_file(self.session_path, context_size as usize);

        match session_load_result {
            Ok(base_tokens) => {
                let base_token_count = base_tokens.len();
                Ok(base_token_count)
            }
            Err(e) => {
                eprintln!(
                    "Warning: Failed to load session file '{}': {}",
                    self.session_path, e
                );
                eprintln!("Falling back to full prompt evaluation (slower)");
                let tokens = model.str_to_token(self.prompt, AddBos::Always)?;

                let mut batch = LlamaCppService::new_batch();
                let batch_limit = LlamaCppService::BATCH_CHUNK_SIZE;

                for (i, token) in tokens.iter().enumerate() {
                    let is_last = i == tokens.len() - 1;
                    batch.add(*token, i as i32, &[0], is_last)?;

                    if batch.n_tokens() >= batch_limit as i32 {
                        ctx.decode(&mut batch)?;
                        batch.clear();
                    }
                }

                if batch.n_tokens() > 0 {
                    ctx.decode(&mut batch)?;
                }
                Ok(tokens.len())
            }
        }
    }

    pub fn append_prompt(
        &self,
        ctx: &mut LlamaContext<'_>,
        model: &LlamaModel,
        dynamic_prompt: &str,
        start_pos: usize,
    ) -> anyhow::Result<(usize, i32)> {
        let dynamic_tokens = model.str_to_token(dynamic_prompt, AddBos::Never)?;

        let mut batch = LlamaCppService::new_batch();
        let batch_limit = LlamaCppService::BATCH_CHUNK_SIZE;
        let mut last_batch_size = 0;

        for (offset, token) in dynamic_tokens.iter().enumerate() {
            let is_last = offset == dynamic_tokens.len() - 1;
            batch.add(*token, (start_pos + offset) as i32, &[0], is_last)?;

            if batch.n_tokens() >= batch_limit as i32 {
                last_batch_size = batch.n_tokens();
                ctx.decode(&mut batch)?;
                batch.clear();
            }
        }

        if batch.n_tokens() > 0 {
            last_batch_size = batch.n_tokens();
            ctx.decode(&mut batch)?;
        }

        let total_tokens = start_pos + dynamic_tokens.len();

        Ok((total_tokens, last_batch_size))
    }

    pub fn delete_current_system_prompt_session(&self) -> anyhow::Result<()> {
        if std::path::Path::new(self.session_path).exists() {
            std::fs::remove_file(self.session_path)?;
            println!("Deleted existing session file to force rebuild");
        }

        if let Some(parent) = std::path::Path::new(self.session_path).parent() {
            std::fs::create_dir_all(parent)?;
            println!("Ensured directory exists: {:?}", parent);
        }
        Ok(())
    }
}
