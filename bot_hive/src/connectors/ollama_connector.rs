use crate::{
    models::user::{HistoryEntry, LLMInput, UserAction},
    services::ollama::OllamaService,
    Env,
};
use std::sync::Arc;

/// Stub implementation for Ollama connector
/// This will eventually replace the llama_cpp connector
pub async fn get_llm_decision(
    env: Arc<Env>,
    _current_input: LLMInput,
    _history: Vec<HistoryEntry>,
) -> UserAction {
    // TODO: Implement Ollama integration using ollama_rs crate
    // Access the OllamaService from env
    let _ollama = &env.ollama;
    
    // TODO: Call ollama service methods to:
    // 1. Build the prompt from history and current input
    // 2. Generate completion using ollama_rs
    // 3. Parse the response
    // 4. Return the decision
    
    eprintln!("[STUB] ollama_connector::get_llm_decision called - not yet implemented");
    UserAction::LLMDecisionResult(Err(
        "Ollama connector not yet implemented".to_string()
    ))
}

