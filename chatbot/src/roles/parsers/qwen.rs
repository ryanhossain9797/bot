
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value as Json;

use super::Parser;
use crate::roles::{ParsedResponse, ParsedToolCall};

static CALL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").expect("CALL_RE is a valid regex")
});
static NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<function=([^>\n]+)>").expect("NAME_RE is a valid regex"));
static PARAM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<parameter=([^>\n]+)>\n(.*?)\n</parameter>")
        .expect("PARAM_RE is a valid regex")
});

pub(super) struct QwenParser;

impl Parser for QwenParser {
    fn parse(&self, raw: &str, close_marker: &str) -> ParsedResponse {
        let (reasoning, body) = raw
            .split_once(close_marker)
            .map_or_else(|| (raw.trim(), ""), |(r, b)| (r.trim(), b));

        let tool_calls = CALL_RE
            .captures_iter(body)
            .filter_map(|call| {
                let inner = &call[1];
                let name = NAME_RE.captures(inner)?[1].trim().to_string();
                let arguments = PARAM_RE
                    .captures_iter(inner)
                    .map(|p| (p[1].trim().to_string(), Json::String(p[2].to_string())))
                    .collect::<serde_json::Map<_, _>>();
                Some(ParsedToolCall {
                    name,
                    arguments: Json::Object(arguments).to_string(),
                })
            })
            .collect();

        let cleaned = CALL_RE.replace_all(body, "");
        let content = cleaned
            .split_once("<tool_call>")
            .map_or(cleaned.as_ref(), |(head, _)| head)
            .trim()
            .to_string();

        ParsedResponse {
            reasoning: reasoning.to_string(),
            content,
            tool_calls,
        }
    }
}
