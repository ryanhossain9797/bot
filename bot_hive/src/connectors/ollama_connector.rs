use crate::{
    models::user::{HistoryEntry, LLMDecisionType, LLMInput, UserAction},
    services::ollama::OllamaService,
    Env,
};
use ollama_rs::generation::chat::ChatMessage;
use serde::Deserialize;
use std::{io::Write, sync::Arc};

#[derive(Debug, Deserialize)]
struct LLMResponse {
    outcome: LLMDecisionType,
}

/// Serialize LLMInput to a user message string
fn serialize_input(input: &LLMInput) -> String {
    match input {
        LLMInput::UserMessage(msg) => msg.clone(),
        LLMInput::ToolResult(result) => {
            format!("Tool Result: {}", result)
        }
    }
}

/// Build the dynamic prompt from history and current input
/// This mirrors the llama_cpp implementation for consistency
fn build_dynamic_prompt(current_input: &LLMInput, history: &[HistoryEntry]) -> String {
    let history_json = serde_json::to_string_pretty(history).unwrap_or_else(|_| "[]".to_string());
    let history_section = format!("Conversation History (JSON):\n{}", history_json);

    let current_input_str = serialize_input(current_input);

    format!(
        "\n{}\n\n{}\n<|im_start|>assistant\n",
        history_section, current_input_str
    )
}

/// Convert history entries to ChatMessage format for Ollama
/// The history alternates between Input and Output
fn history_to_messages(history: &[HistoryEntry]) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    
    for entry in history {
        match entry {
            HistoryEntry::Input(input) => {
                let content = serialize_input(input);
                messages.push(ChatMessage::user(content));
            }
            HistoryEntry::Output(output) => {
                // Convert LLMDecisionType to a string representation
                let content = match output {
                    LLMDecisionType::Final { response } => response.clone(),
                    LLMDecisionType::IntermediateToolCall {
                        maybe_intermediate_response,
                        tool_call,
                    } => {
                        if let Some(response) = maybe_intermediate_response {
                            format!("{} (Tool call: {:?})", response, tool_call)
                        } else {
                            format!("Tool call: {:?}", tool_call)
                        }
                    }
                };
                messages.push(ChatMessage::assistant(content));
            }
        }
    }
    
    messages
}

/// Get response from Ollama service
async fn get_response_from_ollama(
    ollama: &OllamaService,
    current_input: &LLMInput,
    history: &[HistoryEntry],
) -> anyhow::Result<LLMResponse> {
    // Build the full conversation: system prompt + history + current input
    let mut messages = vec![ChatMessage::system(ollama.system_prompt().to_string())];
    
    // Add history messages
    messages.extend(history_to_messages(history));
    
    // Add current input
    let current_input_str = serialize_input(current_input);
    messages.push(ChatMessage::user(current_input_str));
    
    // For Ollama, we need to include the history in the prompt format
    // Since Ollama handles conversation history natively, we can use the messages directly
    // But we also need to include the history JSON in the prompt for context
    // Let's build a combined approach: use the dynamic prompt format in the last user message
    let dynamic_prompt = build_dynamic_prompt(current_input, history);
    
    // Replace the last user message with the formatted prompt
    // The last message should be the current input, replace it with the formatted version
    if !messages.is_empty() {
        let last_idx = messages.len() - 1;
        messages[last_idx] = ChatMessage::user(dynamic_prompt);
    }
    
    // Generate response
    let response_text = ollama.generate(messages).await?;
    
    // Print for debugging (matching llama_cpp behavior)
    print!("{}", response_text);
    println!();
    let _ = std::io::stdout().flush();
    
    // Parse JSON response
    let parsed_response: LLMResponse = serde_json::from_str(&response_text)?;
    
    Ok(parsed_response)
}

pub async fn get_llm_decision(
    env: Arc<Env>,
    current_input: LLMInput,
    history: Vec<HistoryEntry>,
) -> UserAction {
    let ollama_result = get_response_from_ollama(
        env.ollama.as_ref(),
        &current_input,
        &history,
    ).await;

    eprintln!("[DEBUG] ollama_result: {:#?}", ollama_result);

    match ollama_result {
        Ok(ollama_response) => UserAction::LLMDecisionResult(Ok(ollama_response.outcome)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
