use std::sync::Arc;

use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct MessagesRequest {
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip)]
    pub footer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Message {
            role: Role::User,
            content,
        }
    }

    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Message {
            role: Role::Assistant,
            content,
        }
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Message::user(vec![ContentBlock::text(text)])
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Message::assistant(vec![ContentBlock::text(text)])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ContentBlock>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    #[serde(other, skip_serializing)]
    Unknown,
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        ContentBlock::Text { text: text.into() }
    }

    pub fn image(media_type: impl Into<String>, data: Arc<Vec<u8>>) -> Self {
        ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: media_type.into(),
                data,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    Base64 {
        media_type: String,
        #[serde(serialize_with = "ser_base64", deserialize_with = "de_base64")]
        data: Arc<Vec<u8>>,
    },
}

fn ser_base64<S: Serializer>(data: &Arc<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(data.as_slice()))
}

fn de_base64<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<u8>>, D::Error> {
    let encoded = String::deserialize(d)?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map(Arc::new)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesResponse {
    pub id: String,
    pub model: String,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub usage: Usage,
}

impl MessagesResponse {
    pub fn thinking(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    PauseTurn,
    Refusal,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_serializes_to_api_shape() {
        let request = MessagesRequest {
            max_tokens: 1024,
            system: Some("be nice".into()),
            messages: vec![
                Message::user(vec![
                    ContentBlock::text("look"),
                    ContentBlock::image("image/png", Arc::new(vec![1, 2, 3])),
                ]),
                Message::assistant(vec![ContentBlock::ToolUse {
                    id: "call_0".into(),
                    name: "view_image".into(),
                    input: json!({"path": "/tmp/a.png"}),
                }]),
                Message::user(vec![ContentBlock::ToolResult {
                    tool_use_id: "call_0".into(),
                    content: vec![ContentBlock::text("ok")],
                    is_error: false,
                }]),
            ],
            tools: Vec::new(),
            temperature: Some(0.5),
            footer: Some("never serialized".into()),
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            json!({
                "max_tokens": 1024,
                "system": "be nice",
                "messages": [
                    {"role": "user", "content": [
                        {"type": "text", "text": "look"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AQID"}}
                    ]},
                    {"role": "assistant", "content": [
                        {"type": "tool_use", "id": "call_0", "name": "view_image", "input": {"path": "/tmp/a.png"}}
                    ]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "call_0", "content": [{"type": "text", "text": "ok"}]}
                    ]}
                ],
                "temperature": 0.5
            })
        );
    }

    #[test]
    fn response_deserializes_from_api_shape() {
        let response: MessagesResponse = serde_json::from_value(json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude-x",
            "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                {"type": "text", "text": "hello"},
                {"type": "tool_use", "id": "toolu_1", "name": "web_search", "input": {"query": "rust"}}
            ],
            "stop_reason": "tool_use",
            "stop_sequence": null,
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }))
        .unwrap();
        assert_eq!(response.thinking(), "hmm");
        assert_eq!(response.text(), "hello");
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        let calls: Vec<_> = response.tool_uses().collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "web_search");
    }
}
