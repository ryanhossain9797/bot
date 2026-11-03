use std::collections::HashSet;

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
    request.messages = normalize(request.messages);
    WireRequest { model, request }
}

const CONVERSATION_START: &str = "[The conversation so far follows.]";
const EMPTY_MESSAGE: &str = "[empty message]";
const MISSING_RESULT: &str = "[No result was recorded for this tool call.]";
const ORPHANED_RESULT: &str = "[Result of an earlier tool call whose request is no longer in view:]";

fn normalize(messages: Vec<Message>) -> Vec<Message> {
    let mut merged: Vec<Message> = Vec::new();
    for mut message in messages {
        message.content = message.content.into_iter().filter_map(strip_empty).collect();
        match merged.last_mut() {
            Some(last) if last.role == message.role => last.content.extend(message.content),
            _ => merged.push(message),
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut open: Vec<(String, String)> = Vec::new();
    let mut out: Vec<Message> = Vec::with_capacity(merged.len() + 2);
    for mut message in merged {
        match message.role {
            Role::Assistant => {
                if !open.is_empty() {
                    out.push(Message::user(missing_results(&mut open)));
                }
                for block in &mut message.content {
                    if let ContentBlock::ToolUse { id, .. } = block {
                        let wire = unique_id(id, &mut seen);
                        open.push((id.clone(), wire.clone()));
                        *id = wire;
                    }
                }
                out.push(message);
            }
            Role::User => {
                let mut results = Vec::new();
                let mut rest = Vec::new();
                for block in message.content {
                    match block {
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => match open.iter().position(|(original, _)| *original == tool_use_id) {
                            Some(i) => results.push(ContentBlock::ToolResult {
                                tool_use_id: open.remove(i).1,
                                content,
                                is_error,
                            }),
                            None => {
                                rest.push(ContentBlock::text(ORPHANED_RESULT));
                                rest.extend(content);
                            }
                        },
                        other => rest.push(other),
                    }
                }
                results.extend(missing_results(&mut open));
                results.extend(rest);
                message.content = results;
                out.push(message);
            }
        }
    }
    if !open.is_empty() {
        out.push(Message::user(missing_results(&mut open)));
    }

    for message in &mut out {
        if message.content.is_empty() {
            message.content.push(ContentBlock::text(EMPTY_MESSAGE));
        }
    }
    if out.first().is_some_and(|m| m.role == Role::Assistant) {
        out.insert(0, Message::user_text(CONVERSATION_START));
    }
    out
}

fn strip_empty(block: ContentBlock) -> Option<ContentBlock> {
    match block {
        ContentBlock::Text { text } if text.trim().is_empty() => None,
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Some(ContentBlock::ToolResult {
            tool_use_id,
            content: content.into_iter().filter_map(strip_empty).collect(),
            is_error,
        }),
        other => Some(other),
    }
}

fn unique_id(id: &str, seen: &mut HashSet<String>) -> String {
    let mut candidate = id.to_string();
    let mut n = 1;
    while !seen.insert(candidate.clone()) {
        candidate = format!("{id}_{n}");
        n += 1;
    }
    candidate
}

fn missing_results(open: &mut Vec<(String, String)>) -> Vec<ContentBlock> {
    open.drain(..)
        .map(|(_, wire)| ContentBlock::ToolResult {
            tool_use_id: wire,
            content: vec![ContentBlock::text(MISSING_RESULT)],
            is_error: true,
        })
        .collect()
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

    fn tool_use(id: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.into(),
            name: "web_search".into(),
            input: json!({"query": "q"}),
        }
    }

    fn tool_result(id: &str, text: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: vec![ContentBlock::text(text)],
            is_error: false,
        }
    }

    fn roles(messages: &[Message]) -> Vec<Role> {
        messages.iter().map(|m| m.role).collect()
    }

    #[test]
    fn repeated_tool_ids_are_renamed_and_results_follow_them() {
        let out = normalize(vec![
            Message::user_text("go"),
            Message::assistant(vec![tool_use("call_0")]),
            Message::user(vec![tool_result("call_0", "first")]),
            Message::assistant(vec![tool_use("call_0")]),
            Message::user(vec![tool_result("call_0", "second")]),
        ]);
        let value = serde_json::to_value(&out).unwrap();
        assert_eq!(value[1]["content"][0]["id"], "call_0");
        assert_eq!(value[2]["content"][0]["tool_use_id"], "call_0");
        assert_eq!(value[3]["content"][0]["id"], "call_0_1");
        assert_eq!(value[4]["content"][0]["tool_use_id"], "call_0_1");
        assert_eq!(value[4]["content"][0]["content"][0]["text"], "second");
    }

    #[test]
    fn orphaned_result_becomes_text() {
        let out = normalize(vec![
            Message::user(vec![tool_result("gone", "stale")]),
            Message::user_text("hi"),
        ]);
        assert_eq!(out.len(), 1);
        let value = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(value["content"][0]["text"], ORPHANED_RESULT);
        assert_eq!(value["content"][1]["text"], "stale");
        assert_eq!(value["content"][2]["text"], "hi");
    }

    #[test]
    fn unanswered_tool_use_gets_an_error_result_before_other_content() {
        let out = normalize(vec![
            Message::user_text("go"),
            Message::assistant(vec![tool_use("a"), tool_use("b")]),
            Message::user(vec![tool_result("b", "ok")]),
            Message::user_text("followup"),
        ]);
        assert_eq!(roles(&out), [Role::User, Role::Assistant, Role::User]);
        let value = serde_json::to_value(&out[2]).unwrap();
        assert_eq!(value["content"][0]["tool_use_id"], "b");
        assert_eq!(value["content"][1]["tool_use_id"], "a");
        assert_eq!(value["content"][1]["is_error"], true);
        assert_eq!(value["content"][2]["text"], "followup");
    }

    #[test]
    fn tool_use_followed_by_assistant_or_end_still_gets_results() {
        let out = normalize(vec![
            Message::user_text("go"),
            Message::assistant(vec![tool_use("a")]),
            Message::assistant_text("[Your previous turn timed out before it completed.]"),
        ]);
        assert_eq!(roles(&out), [Role::User, Role::Assistant, Role::User]);
        assert_eq!(serde_json::to_value(&out[2]).unwrap()["content"][0]["is_error"], true);
        let out = normalize(vec![
            Message::user_text("go"),
            Message::assistant(vec![tool_use("a")]),
        ]);
        assert_eq!(roles(&out), [Role::User, Role::Assistant, Role::User]);
        assert_eq!(serde_json::to_value(&out[2]).unwrap()["content"][0]["tool_use_id"], "a");
    }

    #[test]
    fn empty_text_is_dropped_and_no_message_is_left_empty() {
        let out = normalize(vec![
            Message::user(vec![ContentBlock::text("  ")]),
            Message::assistant_text("ok"),
            Message::user(Vec::new()),
        ]);
        assert_eq!(roles(&out), [Role::User, Role::Assistant, Role::User]);
        let value = serde_json::to_value(&out).unwrap();
        assert_eq!(value[0]["content"][0]["text"], EMPTY_MESSAGE);
        assert_eq!(value[2]["content"][0]["text"], EMPTY_MESSAGE);
    }

    #[test]
    fn conversation_starting_with_assistant_gets_a_user_turn_first() {
        let out = normalize(vec![
            Message::assistant_text("[Summary of earlier conversation]"),
            Message::user_text("hi"),
        ]);
        assert_eq!(roles(&out), [Role::User, Role::Assistant, Role::User]);
        assert_eq!(
            serde_json::to_value(&out[0]).unwrap()["content"][0]["text"],
            CONVERSATION_START
        );
    }

    #[test]
    fn footer_becomes_user_turn_after_assistant() {
        let body = wire_request("m", request(vec![Message::assistant_text("done")]));
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["messages"][0]["content"][0]["text"], CONVERSATION_START);
        assert_eq!(value["messages"][2]["role"], "user");
        assert_eq!(value["messages"][2]["content"][0]["text"], "FOOTER");
    }
}
