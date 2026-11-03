use serde::Serialize;

use super::anthropic::{ContentBlock, Message, MessagesRequest, MessagesResponse, Role};
use super::{ModelRole, RemoteLlmConfig};

pub struct RemoteLlm {
    config: RemoteLlmConfig,
}

impl RemoteLlm {
    pub fn new(config: RemoteLlmConfig) -> Self {
        RemoteLlm { config }
    }

    fn model(&self, role: ModelRole) -> Option<&'static str> {
        match role {
            ModelRole::Primary => Some(self.config.primary_model),
            ModelRole::Utility => self.config.utility_model,
        }
    }

    pub fn has(&self, role: ModelRole) -> bool {
        self.model(role).is_some()
    }

    pub async fn complete(
        &self,
        role: ModelRole,
        request: MessagesRequest,
    ) -> anyhow::Result<MessagesResponse> {
        let model = self
            .model(role)
            .ok_or_else(|| anyhow::anyhow!("no remote model configured for {role:?}"))?;
        let _body = wire_request(model, request);
        anyhow::bail!(
            "remote LLM backend is not implemented yet (would call {}/v1/messages with model {model})",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

#[derive(Serialize)]
struct WireRequest {
    model: &'static str,
    #[serde(flatten)]
    request: MessagesRequest,
}

fn wire_request(model: &'static str, mut request: MessagesRequest) -> WireRequest {
    if let Some(footer) = request.footer.take() {
        let block = ContentBlock::text(footer);
        match request.messages.last_mut() {
            Some(last) if last.role == Role::User => last.content.push(block),
            _ => request.messages.push(Message::user(vec![block])),
        }
    }
    WireRequest { model, request }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(messages: Vec<Message>) -> MessagesRequest {
        MessagesRequest {
            max_tokens: 100,
            system: Some("sys".into()),
            messages,
            tools: Vec::new(),
            temperature: None,
            footer: Some("FOOTER".into()),
        }
    }

    #[test]
    fn footer_appends_to_last_user_turn() {
        let body = wire_request("m", request(vec![Message::user_text("hi")]));
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({
                "model": "m",
                "max_tokens": 100,
                "system": "sys",
                "messages": [{"role": "user", "content": [
                    {"type": "text", "text": "hi"},
                    {"type": "text", "text": "FOOTER"}
                ]}]
            })
        );
    }

    #[test]
    fn footer_becomes_user_turn_after_assistant() {
        let body = wire_request("m", request(vec![Message::assistant_text("done")]));
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["messages"][1]["role"], "user");
        assert_eq!(value["messages"][1]["content"][0]["text"], "FOOTER");
    }
}
