use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::params::LlamaModelParams,
    model::{AddBos, LlamaModel},
};
use std::num::NonZeroU32;

const SESSION_FILE_PATH: &str = "./resources/base_prompt.session";

pub struct BasePrompt {
    prompt: String,
    session_path: String,
}

impl BasePrompt {
    pub fn new() -> Self {
        Self {
            prompt: Self::build_prompt(),
            session_path: SESSION_FILE_PATH.to_string(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.prompt
    }

    pub fn session_path(&self) -> &str {
        &self.session_path
    }

    fn build_prompt() -> String {
        "<|im_start|>system\nYou are Terminal Alpha and Terminal Beta. Respond with ONLY valid JSON.

RULES:
1. Keep responses 1-3 sentences max
2. No emojis, no markdown
3. Output must be valid JSON

RESPONSE FORMAT:
{\"outcome\":{\"Final\":{\"response\":\"Hello! How can I help you today?\"}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":\"Checking weather for London\",\"tool_call\":{\"GetWeather\":{\"location\":\"London\"}}}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":null,\"tool_call\":{\"GetWeather\":{\"location\":\"Paris\"}}}}}

TOOLS:
- GetWeather: Requires specific location (e.g. \"London\"). If location is vague, ask for clarification in Final response.
- You can make multiple tool calls in separate steps. Make one call, receive the result in history, then make another if needed.

HISTORY:
You receive conversation history as JSON array (oldest to newest). Use it for context.<|im_end|>".to_string()
    }
}

pub fn prepare_llm<'a>() -> anyhow::Result<(LlamaModel, LlamaBackend)> {
    let model_path = std::env::var("MODEL_PATH")
        .unwrap_or_else(|_| "../models/Qwen2.5-14B-Instruct-Q4_K_M.gguf".to_string());

    println!("Loading model from: {}", model_path);

    // Initialize the llama.cpp backend
    let backend = LlamaBackend::init()?;

    // Load the model with default parameters
    let model_params = LlamaModelParams::default();
    let model = LlamaModel::load_from_file(&backend, &model_path, &model_params)?;

    Ok((model, backend))
}

/// Creates a session file with the base prompt pre-evaluated
/// This saves the KV cache for the static system prompt to avoid re-evaluation
pub fn create_session_file(
    model: &LlamaModel,
    backend: &LlamaBackend,
    base_prompt: &str,
    session_path: &str,
) -> anyhow::Result<()> {
    println!("Creating session file at: {}", session_path);

    // Delete existing session file to force rebuild with current context size
    if std::path::Path::new(session_path).exists() {
        std::fs::remove_file(session_path)?;
        println!("Deleted existing session file to force rebuild");
    }

    // Ensure the directory exists
    if let Some(parent) = std::path::Path::new(session_path).parent() {
        std::fs::create_dir_all(parent)?;
        println!("Ensured directory exists: {:?}", parent);
    }

    // Use the same context parameters as runtime - MUST MATCH llm_connector.rs CONTEXT_SIZE
    const CONTEXT_SIZE: u32 = 8192;
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(CONTEXT_SIZE))
        .with_n_threads(num_cpus::get() as i32)
        .with_n_threads_batch(num_cpus::get() as i32);

    let mut ctx = model.new_context(backend, ctx_params)?;

    // Tokenize the base prompt
    let tokens = model.str_to_token(base_prompt, AddBos::Always)?;
    println!("Tokenized base prompt: {} tokens", tokens.len());

    // Add tokens to batch starting at position 0
    let mut batch = LlamaBatch::new(8192, 1);
    for (i, token) in tokens.iter().enumerate() {
        let is_last = i == tokens.len() - 1;
        batch.add(*token, i as i32, &[0], is_last)?;
    }

    // Decode to fill KV cache
    println!("Decoding tokens to fill KV cache...");
    ctx.decode(&mut batch)?;

    // Save session file
    println!("Saving session file...");
    ctx.save_session_file(session_path, &tokens)?;

    // Get file size and log it
    let metadata = std::fs::metadata(session_path)?;
    let file_size_bytes = metadata.len();
    let file_size_mb = file_size_bytes as f64 / (1024.0 * 1024.0);

    println!(
        "Session file created successfully: {} ({} bytes, {:.2} MB)",
        session_path, file_size_bytes, file_size_mb
    );

    Ok(())
}
