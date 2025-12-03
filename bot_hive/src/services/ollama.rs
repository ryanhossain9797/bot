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
const SYSTEM_PROMPT: &str = "<|im_start|>system\nYou are Terminal Alpha and Terminal Beta. Respond with ONLY valid JSON.

RULES:
1. Keep responses brief and to the point.
2. No emojis, no markdown
3. Output must be valid JSON

RESPONSE FORMAT:
{\"outcome\":{\"Final\":{\"response\":\"Hello! How can I help you today?\"}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":\"Checking weather for London\",\"tool_call\":{\"GetWeather\":{\"location\":\"London\"}}}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":null,\"tool_call\":{\"GetWeather\":{\"location\":\"Paris\"}}}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":\"Searching for information about Rust programming\",\"tool_call\":{\"WebSearch\":{\"query\":\"Rust programming language\"}}}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":null,\"tool_call\":{\"WebSearch\":{\"query\":\"latest AI developments 2024\"}}}}}
{\"outcome\":{\"IntermediateToolCall\":{\"maybe_intermediate_response\":\"Calculating 5 + 3 and 4 × 7\",\"tool_call\":{\"MathCalculation\":{\"operations\":[{\"Add\":[5.0, 3.0]}, {\"Mul\":[4.0, 7.0]}]}}}}}

TOOLS (ONLY USE THESE - DO NOT INVENT NEW TOOLS):
- GetWeather: Requires specific location (e.g. \"London\"). If location is vague, ask for clarification in Final response.
- WebSearch: Performs web searches using Brave Search API. Requires a search query string. The tool returns search results with short descriptions only (not full page content). Use this to find current information, look up facts, or research topics. Example queries: \"Rust programming language\", \"weather API documentation\", \"latest news about AI\".
- MathCalculation: Performs mathematical operations. Requires a list of operations. Each operation can be: Add(a, b), Sub(a, b), Mul(a, b), Div(a, b), or Exp(a, b) where a and b are numbers (can be integers or decimals). Example: {\"MathCalculation\":{\"operations\":[{\"Add\":[5.0, 3.0]}, {\"Mul\":[4.5, 7.2]}]}}
- You can make multiple tool calls in separate steps. Make one call, receive the result in history, then make another if needed.
- CRITICAL: Only use GetWeather, WebSearch, or MathCalculation. Never invent other tools.

HISTORY:
You receive conversation history as JSON array (oldest to newest). Use it for context.
It will contain both user messages and tool call results.<|im_end|>";

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
                    eprintln!("Warning: Model '{}' not found. Available models listed above.", OLLAMA_MODEL);
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
        let request = ChatMessageRequest::new(
            self.model.clone(),
            messages,
        )
        .format(FormatType::StructuredJson(Box::new(
            JsonStructure::new::<T>()
        )))
        .options(
            ModelOptions::default()
                .seed(SEED)
                .temperature(TEMPERATURE)
                .num_ctx(CONTEXT_SIZE)
                .num_predict(MAX_GENERATION_TOKENS as i32)
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
