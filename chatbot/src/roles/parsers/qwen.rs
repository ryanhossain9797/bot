//! Parser for the Qwen-family wire format — turns raw generation into reasoning / content / tool
//! calls. This is one model family's output grammar (the `<tool_call><function=…><parameter=…>`
//! pseudo-XML and a reasoning block closed by the model's marker); a role on a different family
//! brings its own parser. Mirrors llama.cpp's parse_response_oaicompat on well-formed output
//! (verified in probe), and degrades safely on truncated output: incomplete tool calls are dropped,
//! never leaked into content. The reasoning `close_marker` is the model's, passed in by the role.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value as Json;

use super::Parser;
use crate::roles::{ParsedResponse, ParsedToolCall};

static CALL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").unwrap());
static NAME_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<function=([^>\n]+)>").unwrap());
static PARAM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<parameter=([^>\n]+)>\n(.*?)\n</parameter>").unwrap());

/// The Qwen-family parser. Zero-sized; held as a static in the parser family.
pub(super) struct QwenParser;

impl Parser for QwenParser {
    fn parse(&self, raw: &str, close_marker: &str) -> ParsedResponse {
        let (reasoning, body) = match raw.split_once(close_marker) {
            Some((r, b)) => (r.trim().to_string(), b),
            None => (raw.trim().to_string(), ""),
        };

        let mut tool_calls = Vec::new();
        for call in CALL_RE.captures_iter(body) {
            let inner = &call[1];
            let Some(name) = NAME_RE.captures(inner).map(|c| c[1].trim().to_string()) else {
                continue;
            };
            let mut args = serde_json::Map::new();
            for p in PARAM_RE.captures_iter(inner) {
                args.insert(p[1].trim().to_string(), Json::String(p[2].to_string()));
            }
            let arguments = Json::Object(args).to_string();
            tool_calls.push(ParsedToolCall { name, arguments });
        }

        let mut content = CALL_RE.replace_all(body, "").into_owned();
        if let Some(idx) = content.find("<tool_call>") {
            content.truncate(idx);
        }
        let content = content.trim().to_string();

        ParsedResponse { reasoning, content, tool_calls }
    }
}
