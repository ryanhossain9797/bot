use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
    token::LlamaToken,
    TokenToStringError,
};
use std::num::NonZero;

const SESSION_FILE_PATH: &str = "./resources/base_prompt.session";
const BASE_PROMPT_IMPL: BasePrompt = BasePrompt::new();

#[derive(Clone, Copy)]
struct BasePrompt {
    prompt: &'static str,
    session_path: &'static str,
}

impl BasePrompt {
    const fn new() -> Self {
        Self {
            prompt: Self::build_prompt(),
            session_path: SESSION_FILE_PATH,
        }
    }

    fn as_str(&self) -> &str {
        self.prompt
    }

    fn session_path(&self) -> &str {
        self.session_path
    }

    const fn build_prompt() -> &'static str {
        "<|im_start|>system\nYou are Terminal Alpha and Terminal Beta. Respond with ONLY valid JSON.

RULES:
1. Keep responses 1-3 sentences max
2. No emojis, no markdown
3. Output must be valid JSON

RESPONSE FORMAT:
{\"outcome\":{\"Final\":{\"response\":\"Hello! How can I help you today?\"}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":\"Checking weather for London\",\"tool_call\":{\"GetWeather\":{\"location\":\"London\"}}}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":null,\"tool_call\":{\"GetWeather\":{\"location\":\"Paris\"}}}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":\"Searching for information about Rust programming\",\"tool_call\":{\"WebSearch\":{\"query\":\"Rust programming language\"}}}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":null,\"tool_call\":{\"WebSearch\":{\"query\":\"latest AI developments 2024\"}}}}}

TOOLS:
- GetWeather: Requires specific location (e.g. \"London\"). If location is vague, ask for clarification in Final response.
- WebSearch: Performs web searches using Brave Search API. Requires a search query string. The tool returns search results with short descriptions only (not full page content). Use this to find current information, look up facts, or research topics. Example queries: \"Rust programming language\", \"weather API documentation\", \"latest news about AI\".
- You can make multiple tool calls in separate steps. Make one call, receive the result in history, then make another if needed.

HISTORY:
You receive conversation history as JSON array (oldest to newest). Use it for context.<|im_end|>"
    }

    fn load_base_prompt(
        &self,
        ctx: &mut LlamaContext,
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

                let mut batch = LlamaBatch::new(LlmService::CONTEXT_SIZE.get() as usize, 1);
                for (i, token) in tokens.iter().enumerate() {
                    let is_last = i == tokens.len() - 1;
                    batch.add(*token, i as i32, &[0], is_last)?;
                }

                ctx.decode(&mut batch)?;
                Ok(tokens.len())
            }
        }
    }

    fn append_prompt(
        &self,
        ctx: &mut LlamaContext,
        model: &LlamaModel,
        dynamic_prompt: &str,
        start_pos: usize,
    ) -> anyhow::Result<usize> {
        let dynamic_tokens = model.str_to_token(dynamic_prompt, AddBos::Never)?;

        let mut batch = LlamaBatch::new(LlmService::CONTEXT_SIZE.get() as usize, 1);

        for (offset, token) in dynamic_tokens.iter().enumerate() {
            let is_last = offset == dynamic_tokens.len() - 1;
            batch.add(*token, (start_pos + offset) as i32, &[0], is_last)?;
        }

        ctx.decode(&mut batch)?;

        let total_tokens = start_pos + dynamic_tokens.len();

        Ok(total_tokens)
    }
}

pub struct LlmService {
    model: LlamaModel,
    backend: LlamaBackend,
    base_prompt: BasePrompt,
}

impl LlmService {
    pub const CONTEXT_SIZE: NonZero<u32> = NonZero::<u32>::new(8192).unwrap();
    pub const MAX_GENERATION_TOKENS: usize = 2000;
    const TEMPERATURE: f32 = 0.25;
    const GRAMMAR_FILE: &'static str = include_str!("../../grammars/response.gbnf");

    pub fn new() -> anyhow::Result<Self> {
        let model_path = std::env::var("MODEL_PATH")
            .unwrap_or_else(|_| "../models/Qwen2.5-14B-Instruct-Q4_K_M.gguf".to_string());

        println!("Loading model from: {}", model_path);

        let backend = LlamaBackend::init()?;

        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, &model_path, &model_params)?;

        let base_prompt = BASE_PROMPT_IMPL;

        // Create session file during construction
        if let Err(e) = Self::create_session_file_impl(
            &model,
            &backend,
            base_prompt.as_str(),
            base_prompt.session_path(),
        ) {
            eprintln!("Warning: Failed to create session file: {}", e);
            eprintln!("The bot will continue without session file caching.");
        }

        Ok(Self {
            model,
            backend,
            base_prompt,
        })
    }

    pub fn context_params() -> LlamaContextParams {
        LlamaContextParams::default()
            .with_n_ctx(Some(Self::CONTEXT_SIZE))
            .with_n_threads(num_cpus::get() as i32)
            .with_n_threads_batch(num_cpus::get() as i32)
    }

    pub fn load_base_prompt(&self, ctx: &mut LlamaContext) -> anyhow::Result<usize> {
        self.base_prompt
            .load_base_prompt(ctx, &self.model, Self::CONTEXT_SIZE.get())
    }

    pub fn append_prompt(
        &self,
        ctx: &mut LlamaContext,
        dynamic_prompt: &str,
        start_pos: usize,
    ) -> anyhow::Result<usize> {
        self.base_prompt
            .append_prompt(ctx, &self.model, dynamic_prompt, start_pos)
    }

    pub fn new_context(&self) -> anyhow::Result<LlamaContext> {
        let ctx_params = Self::context_params();
        Ok(self.model.new_context(&self.backend, ctx_params)?)
    }

    pub fn is_eog_token(&self, token: LlamaToken) -> bool {
        self.model.is_eog_token(token)
    }

    pub fn token_to_str(
        &self,
        token: LlamaToken,
        special: Special,
    ) -> Result<String, TokenToStringError> {
        self.model.token_to_str(token, special)
    }

    pub fn create_sampler(&self) -> LlamaSampler {
        LlamaSampler::chain_simple([
            LlamaSampler::temp(Self::TEMPERATURE),
            LlamaSampler::grammar(&self.model, Self::GRAMMAR_FILE, "root")
                .expect("Failed to load grammar - check GBNF syntax"),
            LlamaSampler::dist(0),
        ])
    }

    fn create_session_file_impl(
        model: &LlamaModel,
        backend: &LlamaBackend,
        base_prompt: &str,
        session_path: &str,
    ) -> anyhow::Result<()> {
        println!("Creating session file at: {}", session_path);

        delete_current_system_prompt_session(session_path)?;

        let ctx_params = Self::context_params();

        let mut ctx = model.new_context(backend, ctx_params)?;

        let tokens = model.str_to_token(base_prompt, AddBos::Always)?;
        println!("Tokenized base prompt: {} tokens", tokens.len());

        let mut batch = LlamaBatch::new(Self::CONTEXT_SIZE.get() as usize, 1);
        for (i, token) in tokens.iter().enumerate() {
            let is_last = i == tokens.len() - 1;
            batch.add(*token, i as i32, &[0], is_last)?;
        }

        println!("Decoding tokens to fill KV cache...");
        ctx.decode(&mut batch)?;

        println!("Saving session file...");
        ctx.save_session_file(session_path, &tokens)?;

        let metadata = std::fs::metadata(session_path)?;
        let file_size_bytes = metadata.len();
        let file_size_mb = file_size_bytes as f64 / (1024.0 * 1024.0);

        println!(
            "Session file created successfully: {} ({} bytes, {:.2} MB)",
            session_path, file_size_bytes, file_size_mb
        );

        Ok(())
    }
}

fn delete_current_system_prompt_session(session_path: &str) -> anyhow::Result<()> {
    if std::path::Path::new(session_path).exists() {
        std::fs::remove_file(session_path)?;
        println!("Deleted existing session file to force rebuild");
    }

    if let Some(parent) = std::path::Path::new(session_path).parent() {
        std::fs::create_dir_all(parent)?;
        println!("Ensured directory exists: {:?}", parent);
    }
    Ok(())
}
