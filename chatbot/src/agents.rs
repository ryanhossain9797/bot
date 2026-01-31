mod executor_agent;
mod thinking_agent;

use std::{
    io::{self, Write},
    ops::ControlFlow,
    sync::Arc,
};

pub use executor_agent::*;
use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
    token::LlamaToken,
};
pub use thinking_agent::*;
use tokio::task::spawn_blocking;

use crate::{
    configuration::debug::{DEBUG_FINAL_LLM_OUTPUT, DEBUG_LIVE_LLM_OUTPUT, DEBUG_LLM_STATS},
    services::llama_cpp::LlamaCppService,
};

struct GenerationState {
    tokens: Vec<LlamaToken>,
    n_cur: usize,
    last_idx: i32,
    sampler: LlamaSampler,
    batch: LlamaBatch<'static>,
}

fn get_response_blocking(
    agent: &'static Agent,
    ctx_params: LlamaContextParams,
    model: Arc<LlamaModel>,
    backend: Arc<LlamaBackend>,
    ctx_size: u32,
    temperature: f32,
    batch_chunk_size: usize,
    dynamic_prompt: String,
) -> anyhow::Result<String> {
    if DEBUG_LLM_STATS {
        print!("[DEBUG] ");
        let _ = io::stdout().flush();
    }

    let mut ctx = model.new_context(backend.as_ref(), ctx_params)?;
    let base_token_count = agent.load(&mut ctx, model.as_ref(), ctx_size, batch_chunk_size)?;

    let (total_tokens, last_batch_size) = agent.append_prompt(
        &mut ctx,
        model.as_ref(),
        &dynamic_prompt,
        base_token_count,
        batch_chunk_size,
    )?;

    if DEBUG_LLM_STATS {
        print!("Total tokens: {total_tokens} ");
        let _ = io::stdout().flush();
    }

    let sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(temperature),
        LlamaSampler::grammar(model.as_ref(), agent.associated_grammar(), "root")
            .expect("Failed to load grammar - check GBNF syntax"),
        LlamaSampler::dist(0),
    ]);

    let initial_prompt_state = GenerationState {
        tokens: Vec::new(),
        n_cur: total_tokens,
        last_idx: last_batch_size - 1,
        sampler: sampler,
        batch: LlamaCppService::new_batch(),
    };

    let max_generation_tokens = LlamaCppService::get_max_generation_tokens();

    let result = (0..max_generation_tokens).try_fold(
        initial_prompt_state,
        |GenerationState {
             mut tokens,
             mut n_cur,
             mut last_idx,
             mut sampler,
             mut batch,
         },
         nth| {
            let token = sampler.sample(&ctx, last_idx);

            match (
                model.token_to_str(token, Special::Tokenize),
                DEBUG_LIVE_LLM_OUTPUT,
            ) {
                (Ok(output), true) => print!("{output}"),
                _ => (),
            }

            if model.is_eog_token(token) {
                return ControlFlow::Break(Ok(tokens));
            }

            tokens.push(token);

            if nth > 0 && nth % (max_generation_tokens / 4) == 0 {
                println!(
                    "{}/4 of limit crossed ({} tokens)",
                    nth / (max_generation_tokens / 4),
                    nth
                );
            }

            match (|| -> anyhow::Result<()> {
                batch.clear();
                batch.add(token, n_cur as i32, &[0], true)?;
                ctx.decode(&mut batch)?;
                Ok(())
            })() {
                Ok(_) => {
                    n_cur += 1;
                    last_idx = batch.n_tokens() - 1;
                    ControlFlow::Continue(GenerationState {
                        tokens,
                        n_cur,
                        last_idx,
                        sampler,
                        batch,
                    })
                }
                Err(e) => ControlFlow::Break(Err(e)),
            }
        },
    );

    let generated_tokens = match result {
        ControlFlow::Continue(GenerationState { tokens, .. }) => Ok(tokens),
        ControlFlow::Break(res) => res,
    }?;

    if DEBUG_LLM_STATS {
        print!("Generated tokens: {} ", generated_tokens.len());
        let _ = io::stdout().flush();
    }

    let mut response_bytes = Vec::new();
    for token in &generated_tokens {
        if let Ok(output) = model.token_to_str(*token, Special::Tokenize) {
            response_bytes.extend_from_slice(output.as_bytes());
        }
    }
    let response = String::from_utf8_lossy(&response_bytes).to_string();

    if DEBUG_FINAL_LLM_OUTPUT {
        println!("\n{}\n", response);
        let _ = std::io::stdout().flush();
    }

    Ok(response)
}

#[derive(Clone, Copy)]
pub struct Agent {
    prompt: &'static str,
    session_path: &'static str,
    associated_grammar: &'static str,
}
impl Agent {
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
        batch_chunk_size: usize,
    ) -> anyhow::Result<usize> {
        let session_load_result = ctx.load_session_file(self.session_path, context_size as usize);

        match session_load_result {
            Ok(base_tokens) => {
                let base_token_count = base_tokens.len();
                Ok(base_token_count)
            }
            Err(e) => {
                eprintln!(
                    "Warning: Failed to load session file '{}': {e:?}",
                    self.session_path,
                );
                eprintln!("Falling back to full prompt evaluation (slower)");
                let tokens = model.str_to_token(self.prompt, AddBos::Always)?;

                let mut batch = LlamaCppService::new_batch();
                let batch_limit = batch_chunk_size;

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
        batch_chunk_size: usize,
    ) -> anyhow::Result<(usize, i32)> {
        let dynamic_tokens = model.str_to_token(dynamic_prompt, AddBos::Never)?;

        let mut batch = LlamaCppService::new_batch();

        let mut last_batch_size = 0;

        for (offset, token) in dynamic_tokens.iter().enumerate() {
            let is_last = offset == dynamic_tokens.len() - 1;
            batch.add(*token, (start_pos + offset) as i32, &[0], is_last)?;

            if batch.n_tokens() >= batch_chunk_size as i32 {
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

    pub async fn get_response(
        &'static self,
        ctx_params: LlamaContextParams,
        model: Arc<LlamaModel>,
        backend: Arc<LlamaBackend>,
        ctx_size: u32,
        temperature: f32,
        batch_chunk_size: usize,
        dynamic_prompt: &str,
    ) -> anyhow::Result<String> {
        let dynamic_prompt = dynamic_prompt.to_string();

        let task = spawn_blocking(move || {
            get_response_blocking(
                self,
                ctx_params,
                Arc::clone(&model),
                Arc::clone(&backend),
                ctx_size,
                temperature,
                batch_chunk_size,
                dynamic_prompt,
            )
        });

        task.await?
    }
}
