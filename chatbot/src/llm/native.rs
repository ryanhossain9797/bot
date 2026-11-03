use std::sync::Arc;

mod chat_format;
mod model_pack;
mod roles;

use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::mtmd::mtmd_default_marker;
use llama_cpp_2::{send_logs_to_tracing, LogOptions};
use serde_json::Value;

use super::anthropic::{
    ContentBlock, ImageSource, Message, MessagesRequest, MessagesResponse, Role as ApiRole,
    StopReason, Tool, Usage,
};
use super::ModelRole;
use self::chat_format::{
    ChatMessage, MessageToolCall, MessageToolCallFunction, ToolDefFunction, ToolDefinition,
};
use self::roles::{PrimaryRole, RenderInputs, Role, UtilityRole};

const DEFAULT_TEMPERATURE: f32 = 1.0;

pub struct NativeLlm {
    primary: Arc<PrimaryRole>,
    utility: Option<Arc<UtilityRole>>,
}

impl NativeLlm {
    pub async fn load() -> anyhow::Result<Self> {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
        let backend = Arc::new(LlamaBackend::init()?);
        let primary = Arc::new(
            tokio::task::spawn_blocking({
                let backend = Arc::clone(&backend);
                move || PrimaryRole::load(backend)
            })
            .await??,
        );

        let utility = match tokio::task::spawn_blocking(move || UtilityRole::load(backend)).await? {
            Ok(role) => {
                println!("[startup] utility model ready");
                Some(Arc::new(role))
            }
            Err(e) => {
                eprintln!("[startup] utility model not loaded: {e:#} — compaction disabled");
                None
            }
        };

        Ok(NativeLlm { primary, utility })
    }

    pub fn bench(message: Option<String>, image_path: Option<String>) -> anyhow::Result<()> {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
        let image = image_path.map(std::fs::read).transpose()?;
        let backend = Arc::new(LlamaBackend::init()?);
        let primary = PrimaryRole::load(backend)?;
        let message = message.unwrap_or_else(|| {
            "Write a Python function that returns the first n prime numbers, then explain how it works."
                .to_string()
        });
        primary.bench(&message, image)
    }

    pub fn has(&self, role: ModelRole) -> bool {
        match role {
            ModelRole::Primary => true,
            ModelRole::Utility => self.utility.is_some(),
        }
    }

    pub async fn complete(
        &self,
        role: ModelRole,
        request: MessagesRequest,
    ) -> anyhow::Result<MessagesResponse> {
        match role {
            ModelRole::Primary => run(Arc::clone(&self.primary), request, true).await,
            ModelRole::Utility => {
                let utility = self
                    .utility
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("utility model is not loaded"))?;
                run(utility, request, false).await
            }
        }
    }
}

async fn run<R: Role>(
    role: Arc<R>,
    request: MessagesRequest,
    log_prompt: bool,
) -> anyhow::Result<MessagesResponse> {
    let marker = mtmd_default_marker();
    let (messages, images) = to_chat_messages(&request.messages, marker);
    let tools: Vec<ToolDefinition> = request.tools.iter().map(to_tool_definition).collect();

    if log_prompt {
        println!("\n\n------------------------ NEW ITERATION ------------------------\n\n");
        println!(
            "{}",
            serde_json::to_string_pretty(&messages).unwrap_or_default()
        );
        if let Some(footer) = &request.footer {
            println!("---- footer ----\n{footer}");
        }
        if !images.is_empty() {
            println!("[image] feeding {} image(s) to the model", images.len());
        }
    }

    let prompt = role
        .render_prompt(&RenderInputs {
            system: request.system.as_deref(),
            messages: &messages,
            tools: (!tools.is_empty()).then_some(tools.as_slice()),
            footer: request.footer.as_deref(),
        })
        .map_err(|e| e.context("prompt render failed"))?;

    let temperature = request.temperature.unwrap_or(DEFAULT_TEMPERATURE);
    let raw = Arc::clone(&role)
        .generate(prompt, images, temperature)
        .await
        .map_err(|e| e.context("generation failed"))?;
    let parsed = role.parse_response(&raw);

    let mut content = Vec::new();
    if !parsed.reasoning.is_empty() {
        content.push(ContentBlock::Thinking {
            thinking: parsed.reasoning,
            signature: String::new(),
        });
    }
    if !parsed.content.is_empty() {
        content.push(ContentBlock::text(parsed.content));
    }
    let has_tool_calls = !parsed.tool_calls.is_empty();
    content.extend(
        parsed
            .tool_calls
            .into_iter()
            .enumerate()
            .map(|(i, call)| ContentBlock::ToolUse {
                id: format!("call_{i}_{}", uuid::Uuid::new_v4().simple()),
                name: call.name,
                input: serde_json::from_str(&call.arguments)
                    .unwrap_or(Value::String(call.arguments)),
            }),
    );

    Ok(MessagesResponse {
        id: format!("native_{}", uuid::Uuid::new_v4()),
        model: role.model_path().display().to_string(),
        role: ApiRole::Assistant,
        content,
        stop_reason: Some(if has_tool_calls {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        }),
        usage: Usage::default(),
    })
}

#[cfg(test)]
pub(crate) fn render_with_pack(
    pack_dir: &std::path::Path,
    request: &MessagesRequest,
) -> anyhow::Result<(String, Vec<Arc<Vec<u8>>>)> {
    let pack = model_pack::Pack::load_from(pack_dir)?;
    let (messages, images) = to_chat_messages(&request.messages, mtmd_default_marker());
    let tools: Vec<ToolDefinition> = request.tools.iter().map(to_tool_definition).collect();
    let prompt = roles::render_template(
        &pack.template,
        &RenderInputs {
            system: request.system.as_deref(),
            messages: &messages,
            tools: (!tools.is_empty()).then_some(tools.as_slice()),
            footer: request.footer.as_deref(),
        },
        roles::FormatFlags {
            enable_thinking: pack.manifest.format.enable_thinking,
            add_generation_prompt: pack.manifest.format.add_generation_prompt,
        },
    )?;
    Ok((prompt, images))
}

fn to_tool_definition(tool: &Tool) -> ToolDefinition {
    ToolDefinition {
        kind: "function",
        function: ToolDefFunction {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        },
    }
}

fn part(block: &ContentBlock, marker: &str, images: &mut Vec<Arc<Vec<u8>>>) -> Option<String> {
    match block {
        ContentBlock::Text { text } => Some(text.clone()),
        ContentBlock::Image {
            source: ImageSource::Base64 { data, .. },
        } => {
            images.push(Arc::clone(data));
            Some(marker.to_string())
        }
        ContentBlock::ToolUse { .. }
        | ContentBlock::ToolResult { .. }
        | ContentBlock::Thinking { .. }
        | ContentBlock::Unknown => None,
    }
}

fn to_chat_messages(
    messages: &[Message],
    marker: &str,
) -> (Vec<ChatMessage>, Vec<Arc<Vec<u8>>>) {
    let mut out = Vec::new();
    let mut images = Vec::new();

    for message in messages {
        match message.role {
            ApiRole::Assistant => {
                let mut parts = Vec::new();
                let mut calls = Vec::new();
                for block in &message.content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        calls.push(MessageToolCall {
                            id: id.clone(),
                            kind: "function",
                            function: MessageToolCallFunction {
                                name: name.clone(),
                                arguments: input.as_object().cloned().unwrap_or_default(),
                            },
                        });
                    } else if let Some(p) = part(block, marker, &mut images) {
                        parts.push(p);
                    }
                }
                out.push(ChatMessage::assistant_with_tools(parts.join("\n"), calls));
            }
            ApiRole::User => {
                let mut parts: Vec<String> = Vec::new();
                let mut emitted = false;
                for block in &message.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                    {
                        if !parts.is_empty() {
                            out.push(ChatMessage::user(parts.join("\n")));
                            parts.clear();
                        }
                        let result: Vec<String> = content
                            .iter()
                            .filter_map(|b| part(b, marker, &mut images))
                            .collect();
                        out.push(ChatMessage::tool(tool_use_id.clone(), result.join("\n")));
                        emitted = true;
                    } else if let Some(p) = part(block, marker, &mut images) {
                        parts.push(p);
                    }
                }
                if !parts.is_empty() || !emitted {
                    out.push(ChatMessage::user(parts.join("\n")));
                }
            }
        }
    }

    (out, images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::chat_format::ChatRole;
    use serde_json::json;

    #[test]
    fn maps_blocks_back_to_template_messages() {
        let img = Arc::new(vec![9u8]);
        let messages = vec![
            Message::user(vec![
                ContentBlock::text("hi"),
                ContentBlock::text("url: u"),
                ContentBlock::image("image/png", Arc::clone(&img)),
            ]),
            Message::assistant(vec![ContentBlock::ToolUse {
                id: "call_0".into(),
                name: "web_search".into(),
                input: json!({"query": "q"}),
            }]),
            Message::user(vec![ContentBlock::ToolResult {
                tool_use_id: "call_0".into(),
                content: vec![ContentBlock::text("result")],
                is_error: false,
            }]),
            Message::user(vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: Vec::new(),
                is_error: false,
            }]),
            Message::user_text("followup"),
        ];
        let (chat, images) = to_chat_messages(&messages, "<M>");
        assert_eq!(images.len(), 1);
        assert_eq!(chat.len(), 5);
        assert!(matches!(chat[0].role, ChatRole::User));
        assert_eq!(chat[0].content, "hi\nurl: u\n<M>");
        assert!(matches!(chat[1].role, ChatRole::Assistant));
        assert_eq!(chat[1].content, "");
        assert_eq!(chat[1].tool_calls[0].function.name, "web_search");
        assert!(matches!(chat[2].role, ChatRole::Tool));
        assert_eq!(chat[2].tool_call_id.as_deref(), Some("call_0"));
        assert_eq!(chat[2].content, "result");
        assert_eq!(chat[3].content, "");
        assert_eq!(chat[4].content, "followup");
    }

    #[test]
    fn mixed_user_message_splits_around_tool_results() {
        let messages = vec![Message::user(vec![
            ContentBlock::ToolResult {
                tool_use_id: "a".into(),
                content: vec![ContentBlock::text("r")],
                is_error: false,
            },
            ContentBlock::text("after"),
        ])];
        let (chat, _) = to_chat_messages(&messages, "<M>");
        assert_eq!(chat.len(), 2);
        assert!(matches!(chat[0].role, ChatRole::Tool));
        assert!(matches!(chat[1].role, ChatRole::User));
        assert_eq!(chat[1].content, "after");
    }
}
