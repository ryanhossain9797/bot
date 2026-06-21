mod primary_role;

use std::io;
use std::path::Path;

use minijinja::{value::Value, Environment, Error, ErrorKind};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use serde_json::Value as Json;

pub use primary_role::PrimaryRole;

/// A role owns the identity/format contract (system prompt, footer placement, sampling temperature)
/// and the path to its model pack, and turns conversation data into a prompt and raw output back
/// into structured pieces. Everything model-specific lives in files under the pack — the role only
/// holds the loaded template + the pack path.
pub trait Role: Send + Sync {
    fn system_prompt(&self) -> &str;
    fn temperature(&self) -> f32;
    /// Path to this role's model pack. Part of the Role contract; reserved for diagnostics and
    /// multi-model selection.
    #[allow(dead_code)]
    fn model_dir(&self) -> &Path;
    /// Render the final prompt string from conversation inputs (option-b: JSON rendered in Rust).
    fn render_prompt(&self, inputs: &RenderInputs) -> anyhow::Result<String>;
    /// Parse raw generation into reasoning / content / tool calls.
    fn parse_response(&self, raw: &str) -> ParsedResponse;
}

pub struct RenderInputs<'a> {
    /// Conversation messages (history + live), without the system prompt or footer.
    pub messages: &'a Json,
    /// Tool definitions (array), or `None` when tools are disabled this turn.
    pub tools: Option<&'a Json>,
    /// Dynamic metadata footer; the role places it (a final system block before the gen prompt).
    pub footer: Option<&'a str>,
}

/// Model-format render flags, sourced from the pack manifest `[format]` section.
#[derive(Clone, Copy)]
pub struct FormatFlags {
    pub enable_thinking: bool,
    pub add_generation_prompt: bool,
}

pub struct ParsedToolCall {
    pub name: String,
    /// Arguments as a JSON object string, ready for `ToolType::bind`.
    pub arguments: String,
}

pub struct ParsedResponse {
    pub reasoning: String,
    pub content: String,
    pub tool_calls: Vec<ParsedToolCall>,
}

// ───────────────────────────── render ─────────────────────────────

// minja (llama.cpp) serializes JSON like Python's json.dumps: ", " / ": " separators, insertion
// order, no HTML escaping. This formatter (+ the preserve_order feature) reproduces it, keeping the
// tools block in the model's training distribution.
struct Spaced;
impl serde_json::ser::Formatter for Spaced {
    fn begin_array_value<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if first { Ok(()) } else { w.write_all(b", ") }
    }
    fn begin_object_key<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        if first { Ok(()) } else { w.write_all(b", ") }
    }
    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }
}

fn json_spaced(value: &Json) -> String {
    let mut buf = Vec::new();
    value
        .serialize(&mut serde_json::Serializer::with_formatter(&mut buf, Spaced))
        .expect("serializing to a Vec is infallible");
    String::from_utf8(buf).expect("serde_json emits valid utf-8")
}

// Pre-serialize each tool definition to a JSON string; the template emits it verbatim.
fn prepare_tools(tools: &Json) -> Vec<String> {
    tools.as_array().map(|a| a.iter().map(json_spaced).collect()).unwrap_or_default()
}

// Normalize each tool call's `arguments` into an object whose values are all strings, so the
// template can `|items` over it and splice each value directly. `arguments` may arrive either as
// the OpenAI wire form (a JSON string, as stored history does) or as an object; both are handled.
// String values stay raw, everything else becomes spaced-JSON.
fn normalize_tool_args(messages: &mut Json) {
    for msg in messages.as_array_mut().into_iter().flatten() {
        let Some(tool_calls) = msg.get_mut("tool_calls").and_then(|v| v.as_array_mut()) else {
            continue;
        };
        for tc in tool_calls {
            let Some(field) = tc.pointer_mut("/function/arguments") else {
                continue;
            };
            // Coerce to a map: parse the wire string, or take the object as-is.
            let map = match field {
                Json::String(s) => match serde_json::from_str::<Json>(s) {
                    Ok(Json::Object(m)) => m,
                    _ => continue,
                },
                Json::Object(m) => std::mem::take(m),
                _ => continue,
            };
            let normalized = map
                .into_iter()
                .map(|(k, v)| {
                    let s = v.as_str().map(str::to_string).unwrap_or_else(|| json_spaced(&v));
                    (k, Json::String(s))
                })
                .collect();
            *field = Json::Object(normalized);
        }
    }
}

fn render(
    template: &str,
    system_prompt: &str,
    inputs: &RenderInputs,
    flags: FormatFlags,
) -> anyhow::Result<String> {
    let mut msgs = vec![serde_json::json!({ "role": "system", "content": system_prompt })];
    if let Some(arr) = inputs.messages.as_array() {
        msgs.extend(arr.iter().cloned());
    }
    let mut messages = Json::Array(msgs);
    normalize_tool_args(&mut messages);

    let tools = inputs.tools.map(prepare_tools);

    let mut env = Environment::new();
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    env.add_function("raise_exception", |msg: String| -> Result<Value, Error> {
        Err(Error::new(ErrorKind::InvalidOperation, msg))
    });
    env.add_template("chat", template)?;
    let tmpl = env.get_template("chat")?;
    let rendered = tmpl.render(minijinja::context! {
        messages => Value::from_serialize(&messages),
        tools => tools.map_or(Value::UNDEFINED, |t| Value::from_serialize(&t)),
        footer => inputs.footer,
        add_generation_prompt => flags.add_generation_prompt,
        enable_thinking => flags.enable_thinking,
    })?;
    Ok(rendered)
}

// ───────────────────────────── parse ──────────────────────────────

static CALL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").unwrap());
static NAME_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<function=([^>\n]+)>").unwrap());
static PARAM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<parameter=([^>\n]+)>\n(.*?)\n</parameter>").unwrap());

// Parse the raw generation into reasoning / content / tool calls. The prompt primes `<think>\n`, so
// the output starts inside the think block. Mirrors llama.cpp's parse_response_oaicompat on every
// well-formed shape (verified in probe), and degrades safely on truncated output: incomplete tool
// calls are dropped and never leaked into content.
fn parse(raw: &str) -> ParsedResponse {
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
