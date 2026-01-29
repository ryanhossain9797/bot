use llama_cpp_2::{
    context::{params::LlamaContextParams, LlamaContext},
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel, Special},
    sampling::LlamaSampler,
    token::LlamaToken,
    TokenToStringError,
};
use llama_cpp_2::{send_logs_to_tracing, LogOptions};
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
        r#"<|im_start|>system\nYour name is Terminal Alpha Beta. Respond with ONLY valid JSON.

RUST TYPE DEFINITIONS:
```rust
pub enum LLMDecisionType {
    IntermediateToolCall { tool_call: ToolCall },
    InternalFunctionCall { function_call: FunctionCall },
    MessageUser { response: String },
}

pub struct LLMResponse {
    pub thoughts: String,
    pub outcome: LLMDecisionType,
}

pub enum MathOperation {
    Add(f32, f32),
    Sub(f32, f32),
    Mul(f32, f32),
    Div(f32, f32),
    Exp(f32, f32),
}

pub enum ToolCall {
    /// IMPORTANT: Do not use this tool without the user's specific City.
    GetWeather { location: String },
    /// IMPORTANT: You SHOULD USUALLY follow up this tool call with a VisitUrl call to read the actual content of the found pages.
    WebSearch { query: String },
    MathCalculation { operations: Vec<MathOperation> },
    /// Visit a URL and extract its content. Use this to read the full content of pages found via WebSearch IF NEEDED.
    VisitUrl { url: String },
}

pub enum FunctionCall {
    /// Use this to recall recent UNTRUNCATED conversation history (last 20 messages). Use RecallLongTerm if this doesn't provide useful results.
    RecallShortTerm { reason: String },
    /// Keep search_term SHORT for maximum coverage. Opt to use this as often as possible if necessary.
    RecallLongTerm { search_term: String },
}
```

RULES:
Your response needs to match LLMResponse type's JSON Serialization exactly.
Keep responses brief and to the point.
Use RecallLongTerm and RecallShortTerm often to try and be helpful. use the alternative if one does not yield useful results.

DECISION MAKING:
If you have enough information from thoughts to answer the user request, use "MessageUser".
If you need more information from the user themselves, use "MessageUser" too, like getting city for weather when they don't specify it.
If you have to perform an action, use "IntermediateToolCall" or "InternalFunctionCall".

THOUGHTS FIELD USAGE:
The 'thoughts' field is CRITICAL for maintaining state across multiple turns.
TRACK ATTEMPTS: Explicitly track failures and retries. E.g., "Attempt 1/3 failed. Trying new query..."
Include summaries of information gathered so far in 'thoughts' so you don't lose it.
This field is your PRIMARY memory. Use it to keep all information you might need in subsequent runs.
Example of thoughts

Thoughts while information retrieval is in progress
```
User has asked me to fetch the weather of dhaka and london and then compare which is higher.
[x] Fetch weather for dhaka. DONE: weather is 31.5 degrees
[ ] Fetch weather for london.
[ ] Compare weather to tell user which is higher
```

Thoughts after all work is done all information collected
```
I have completed fetching weather for dhaka and london and comparing them
[x] Fetch weather for dhaka. DONE: weather is 31.5 degrees
[x] Fetch weather for london. DONE: weather is 27.5 degrees
[x] Compare weather to tell user which is higher. DONE: dhaka is higher
I will notify the user
```

CRITICAL INSTRUCTIONS:
IntermediateToolCall and InternalFunctionCall are functionally EQUIVALENT, They have been partitioned only to distinguish which is considered your internal monlogue vs using an external tool.
Heavily rely on RecallLongTerm and RecallShortTerm, especialy whenever user implies you're supposed to know something. Or even when you think you might know something from earlier in the conversation.
If necessary use RecallLongTerm again with information you gained from the first recall(s).
Keep RecallLongTerm search terms SHORT for maximum coverage.
WebSearch tool ONLY gives you a summary. To answer the user's question, you ALMOST ALWAYS need to read the page content using VisitUrl.
Use thoughts to keep track of important details accross tool calls and user interactions.
You can make multiple tool calls in separate steps. Make one call, commit the result in thoughts, then make another if needed.
If you need to refer to earlier parts of the ongoing conversation, use the RecallShortTerm internal function to retrieve the last 20 messages.
<|im_end|>"#
    }

    fn load_base_prompt(
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

    fn append_prompt(
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
}

pub struct LlamaCppService {
    model: LlamaModel,
    backend: LlamaBackend,
    base_prompt: BasePrompt,
}

impl LlamaCppService {
    const CONTEXT_SIZE: NonZero<u32> = NonZero::<u32>::new(32768).unwrap();
    pub const BATCH_CHUNK_SIZE: usize = 2048;
    const MAX_GENERATION_TOKENS: usize = 8192;
    const TEMPERATURE: f32 = 0.25;
    const GRAMMAR_FILE: &'static str = include_str!("../../grammars/response.gbnf");

    pub const fn get_max_generation_tokens() -> usize {
        Self::MAX_GENERATION_TOKENS
    }

    pub fn new() -> anyhow::Result<Self> {
        let model_path = std::env::var("MODEL_PATH")
            .unwrap_or_else(|_| "./models/GLM-4-32B-0414-Q8_0-matteov2.gguf".to_string());

        println!("Loading model from: {}", model_path);

        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
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
            .with_n_batch(4096)
            .with_n_threads(num_cpus::get() as i32)
            .with_n_threads_batch(num_cpus::get() as i32)
    }

    pub fn load_base_prompt(&self, ctx: &mut LlamaContext<'_>) -> anyhow::Result<usize> {
        self.base_prompt
            .load_base_prompt(ctx, &self.model, Self::CONTEXT_SIZE.get())
    }

    pub fn append_prompt(
        &self,
        ctx: &mut LlamaContext<'_>,
        dynamic_prompt: &str,
        start_pos: usize,
    ) -> anyhow::Result<(usize, i32)> {
        self.base_prompt
            .append_prompt(ctx, &self.model, dynamic_prompt, start_pos)
    }

    pub fn new_context(&self) -> anyhow::Result<LlamaContext<'_>> {
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

    pub fn new_batch() -> LlamaBatch<'static> {
        LlamaBatch::new(Self::CONTEXT_SIZE.get() as usize, 1)
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

        let mut batch = Self::new_batch();
        let batch_limit = Self::BATCH_CHUNK_SIZE;

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
