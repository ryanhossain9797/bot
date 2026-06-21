mod primary_role;

use std::io;
use std::path::Path;

use minijinja::{value::Value, Environment, Error, ErrorKind};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use serde_json::Value as Json;

use crate::tools::ToolDefinition;
use crate::types::conversation::ChatMessage;

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
    /// How an over-long reasoning block is force-closed during generation.
    fn thinking(&self) -> ThinkingPolicy;
}

/// The reasoning-block policy the generation loop enforces. Composed by the role from its own
/// nudge prose and the model's `close_marker` (sourced from the pack manifest).
#[derive(Clone)]
pub struct ThinkingPolicy {
    /// Text injected to force-close reasoning once the budget is hit (ends with `close_marker`).
    pub force_close: String,
    /// The model's reasoning close marker, e.g. `</think>`.
    pub close_marker: String,
    /// Token budget after which `force_close` is injected.
    pub max_tokens: usize,
}

pub struct RenderInputs<'a> {
    /// Conversation messages (history + live), without the system prompt or footer.
    pub messages: &'a [ChatMessage],
    /// Tool definitions, or `None` when tools are disabled this turn.
    pub tools: Option<&'a [ToolDefinition]>,
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

fn json_spaced<T: Serialize>(value: &T) -> String {
    let mut buf = Vec::new();
    value
        .serialize(&mut serde_json::Serializer::with_formatter(&mut buf, Spaced))
        .expect("serializing to a Vec is infallible");
    String::from_utf8(buf).expect("serde_json emits valid utf-8")
}

fn prepare_tools(tools: &[ToolDefinition]) -> Vec<String> {
    tools.iter().map(json_spaced).collect()
}

fn render(
    template: &str,
    system_prompt: &str,
    inputs: &RenderInputs,
    flags: FormatFlags,
) -> anyhow::Result<String> {
    let mut messages = Vec::with_capacity(inputs.messages.len() + 1);
    messages.push(ChatMessage::system(system_prompt));
    messages.extend(inputs.messages.iter().cloned());

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


static CALL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").unwrap());
static NAME_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<function=([^>\n]+)>").unwrap());
static PARAM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<parameter=([^>\n]+)>\n(.*?)\n</parameter>").unwrap());

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

    let mut content = CALL_RE.replace_all(body, "").into_owned();
    if let Some(idx) = content.find("<tool_call>") {
        content.truncate(idx);
    }
    let content = content.trim().to_string();

    ParsedResponse { reasoning, content, tool_calls }
}
