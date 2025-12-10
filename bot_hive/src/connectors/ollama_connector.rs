use crate::{
    models::user::{HistoryEntry, LLMDecisionType, LLMInput, UserAction},
    services::ollama::OllamaService,
    Env,
};
use ollama_rs::generation::{chat::ChatMessage, parameters::JsonSchema};
use serde::Deserialize;
use std::{io::Write, sync::Arc};

#[derive(Debug, Deserialize, JsonSchema)]
struct LLMResponse {
    outcome: LLMDecisionType,
}

/// Format tool call as a simple string
fn format_tool_call(tool_call: &crate::models::user::ToolCall) -> String {
    match tool_call {
        crate::models::user::ToolCall::GetWeather { location } => {
            format!("GetWeather: location=\"{}\"", location)
        }
        crate::models::user::ToolCall::WebSearch { query } => {
            format!("WebSearch: query=\"{}\"", query)
        }
        crate::models::user::ToolCall::MathCalculation { operations } => {
            format!("MathCalculation: {} operations", operations.len())
        }
        crate::models::user::ToolCall::VisitUrl { url } => {
            format!("VisitUrl: url=\"{}\"", url)
        }
    }
}

/// Convert history entries to simple line-based format for Ollama
/// Format: USER: <message> or TOOL RESPONSE: <result>
///         ASSISTANT FINAL: "..." or ASSISTANT TOOL CALL: Response: <response> Tool: <tool>
fn history_to_messages(history: &[HistoryEntry]) -> Vec<ChatMessage> {
    let mut messages = Vec::new();

    for entry in history {
        match entry {
            HistoryEntry::Input(input) => {
                let content = match input {
                    LLMInput::UserMessage(msg) => format!("USER: {}", msg),
                    LLMInput::ToolResult(result) => format!("TOOL RESPONSE: {}", result),
                };
                messages.push(ChatMessage::user(content));
            }
            HistoryEntry::Output(output) => {
                let content = match output {
                    LLMDecisionType::Final { response } => {
                        format!("ASSISTANT FINAL: \"{}\"", response)
                    }
                    LLMDecisionType::IntermediateToolCall {
                        maybe_intermediate_response,
                        tool_call,
                    } => {
                        let response_part = match maybe_intermediate_response {
                            Some(r) if !r.is_empty() => format!("\"{}\"", r),
                            _ => "null".to_string(),
                        };
                        let tool_part = format_tool_call(tool_call);
                        format!(
                            "ASSISTANT TOOL CALL: Response: {} Tool: {}",
                            response_part, tool_part
                        )
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

    // Add history messages in simple line-based format
    messages.extend(history_to_messages(history));

    // Add current input in simple format
    let current_input_str = match current_input {
        LLMInput::UserMessage(msg) => format!("USER: {}", msg),
        LLMInput::ToolResult(result) => format!("TOOL RESPONSE: {}", result),
    };
    messages.push(ChatMessage::user(current_input_str));

    // Generate response with structured JSON schema to enforce valid tool calls
    let response_text = ollama.generate::<LLMResponse>(messages).await?;

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
    let ollama_result =
        get_response_from_ollama(env.ollama.as_ref(), &current_input, &history).await;

    eprintln!("[DEBUG] ollama_result: {:#?}", ollama_result);

    match ollama_result {
        Ok(ollama_response) => UserAction::LLMDecisionResult(Ok(ollama_response.outcome)),
        Err(err) => UserAction::LLMDecisionResult(Err(err.to_string())),
    }
}
