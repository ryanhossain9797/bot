//! Response decoding — turn raw generation into reasoning / content / tool calls. Mirrors
//! llama.cpp's parse_response_oaicompat on well-formed output (verified in probe), and degrades
//! safely on truncated output: incomplete tool calls are dropped, never leaked into content.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value as Json;

use super::{ParsedResponse, ParsedToolCall};

static CALL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").unwrap());
static NAME_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<function=([^>\n]+)>").unwrap());
static PARAM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<parameter=([^>\n]+)>\n(.*?)\n</parameter>").unwrap());

pub(super) fn parse(raw: &str) -> ParsedResponse {
    // The prompt primes `<think>\n`, so the output starts inside the think block: reasoning runs
    // up to the first `</think>`; if it never closes, the whole output is reasoning.
    let (reasoning, body) = match raw.split_once("</think>") {
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

    // Content is the body minus complete tool-call blocks; a dangling (truncated) `<tool_call>`
    // is dropped, never shown to the user as raw text.
    let mut content = CALL_RE.replace_all(body, "").into_owned();
    if let Some(idx) = content.find("<tool_call>") {
        content.truncate(idx);
    }
    let content = content.trim().to_string();

    ParsedResponse { reasoning, content, tool_calls }
}
