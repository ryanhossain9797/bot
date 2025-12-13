use ollama_rs::{
    generation::{
        chat::{request::ChatMessageRequest, ChatMessage},
        parameters::{FormatType, JsonStructure, KeepAlive, TimeUnit},
    },
    models::ModelOptions,
    Ollama,
};
use std::sync::Arc;

const OLLAMA_HOST: &str = "http://localhost:11434";
const OLLAMA_PORT: u16 = 11434;
const OLLAMA_MODEL: &str = "qwen2.5:14b"; // Matches the model in Dockerfile.base
const TEMPERATURE: f32 = 0.25; // Same as llama_cpp
const MAX_GENERATION_TOKENS: usize = 2000; // Same as llama_cpp
const CONTEXT_SIZE: u64 = 8192; // Same as llama_cpp
const SEED: i32 = 42; // Fixed seed for deterministic responses

// System prompt from llama_cpp - shared across all requests
const SYSTEM_PROMPT: &str = r#"<|im_start|>system
Your name is Terminal Alpha Beta. Respond with ONLY valid JSON.

RULES:
1. Keep responses brief and to the point.
2. NO HTML TAGS. Plain text only.
3. No emojis, no markdown.
4. Output must be valid JSON.

RESPONSE FORMAT:
{"outcome":{"Final":{"response":"Hello! How can I help you today?"}}}
{"outcome":{"IntermediateToolCall":{"maybe_intermediate_response":"Checking weather for London","tool_call":{"GetWeather":{"location":"London"}}}}}

TOOLS (RUST TYPE DEFINITIONS):
```rust
pub enum MathOperation {
    Add(f32, f32),
    Sub(f32, f32),
    Mul(f32, f32),
    Div(f32, f32),
    Exp(f32, f32),
}

pub enum ToolCall {
    GetWeather { location: String },
    /// IMPORTANT: You SHOULD USUALLY follow up this tool call with a VisitUrl call to read the actual content of the found pages.
    WebSearch { query: String },
    MathCalculation { operations: Vec<MathOperation> },
    /// Visit a URL and extract its content. Use this to read the full content of pages found via WebSearch IF NEEDED.
    VisitUrl { url: String },
}
```

CRITICAL INSTRUCTIONS:
- ONLY use the tools defined above.
- WebSearch ONLY gives you a summary. To answer the user's question, you ALMOST ALWAYS need to read the page content using VisitUrl.
- Do not invent new tools.

HISTORY:
You receive conversation history as JSON array (oldest to newest). Use it for context.
It will contain both user messages and tool call results.<|im_end|>"#;

/// Ollama service for LLM inference using ollama_rs crate
/// This replaces the llama_cpp service
///
/// The Ollama client is Send + Sync (it's an HTTP client wrapper)
pub struct OllamaService {
    client: Arc<Ollama>,
    model: String,
}

impl OllamaService {
    pub async fn new() -> anyhow::Result<Self> {
        let client = Arc::new(Ollama::new(OLLAMA_HOST.to_string(), OLLAMA_PORT));

        // Verify connection by listing models
        match client.list_local_models().await {
            Ok(models) => {
                println!("Connected to Ollama! Available models:");
                for model in &models {
                    println!("  - {}", model.name);
                }

                // Verify our model is available
                if models.iter().any(|m| m.name == OLLAMA_MODEL) {
                    println!("Model '{}' is available.", OLLAMA_MODEL);
                } else {
                    eprintln!(
                        "Warning: Model '{}' not found. Available models listed above.",
                        OLLAMA_MODEL
                    );
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to list Ollama models: {}", e);
                eprintln!("Ollama may not be running yet. The service will continue but requests may fail.");
            }
        }

        Ok(Self {
            client,
            model: OLLAMA_MODEL.to_string(),
        })
    }

    /// Get the system prompt (shared across all requests)
    pub fn system_prompt(&self) -> &'static str {
        SYSTEM_PROMPT
    }

    /// Get the model name
    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// Generate a completion given a conversation history with structured JSON format
    /// The conversation should include the system prompt as the first message
    /// Uses structured JSON format to enforce valid tool calls based on the schema
    pub async fn generate<T: ollama_rs::generation::parameters::JsonSchema>(
        &self,
        messages: Vec<ChatMessage>,
    ) -> anyhow::Result<String> {
        let request = ChatMessageRequest::new(self.model.clone(), messages)
            .format(FormatType::StructuredJson(Box::new(
                JsonStructure::new::<T>(),
            )))
            .options(
                ModelOptions::default()
                    .seed(SEED)
                    .temperature(TEMPERATURE)
                    .num_ctx(CONTEXT_SIZE)
                    .num_predict(MAX_GENERATION_TOKENS as i32),
            )
            .keep_alive(KeepAlive::Until {
                time: 30,
                unit: TimeUnit::Minutes,
            });

        let response = self.client.send_chat_messages(request).await?;
        Ok(response.message.content)
    }

    /// Generate a simple text completion without structured JSON
    /// Used for tasks like summarization or content extraction
    pub async fn generate_simple(&self, messages: Vec<ChatMessage>) -> anyhow::Result<String> {
        let request = ChatMessageRequest::new(self.model.clone(), messages)
            .options(
                ModelOptions::default()
                    .seed(SEED)
                    .temperature(TEMPERATURE)
                    .num_ctx(CONTEXT_SIZE)
                    .num_predict(MAX_GENERATION_TOKENS as i32),
            )
            .keep_alive(KeepAlive::Until {
                time: 30,
                unit: TimeUnit::Minutes,
            });

        let response = self.client.send_chat_messages(request).await?;
        Ok(response.message.content)
    }

    /// Get a reference to the Ollama client (for advanced use cases)
    pub fn client(&self) -> &Arc<Ollama> {
        &self.client
    }
}
