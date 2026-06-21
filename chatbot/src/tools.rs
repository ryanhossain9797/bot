use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use strum::IntoEnumIterator;

use crate::types::conversation::{ToolKind, ToolType};

/// A tool definition in the shape the chat template serializes into the `<tools>` block. The
/// `parameters` JSON schema is genuinely dynamic, so it stays a `Value`; the rest is typed.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: ToolDefFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefFunction {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
}

#[derive(Debug, Deserialize)]
struct VisitUrlArgs {
    url: String,
}

#[derive(Debug, Deserialize)]
struct RunBashArgs {
    command: String,
}

#[derive(Debug, Deserialize)]
struct PathArgs {
    path: String,
}

fn parse_args<T: serde::de::DeserializeOwned>(name: &str, arguments: &str) -> anyhow::Result<T> {
    serde_json::from_str(arguments)
        .map_err(|e| anyhow::anyhow!("{name} arguments failed to bind: {e} — raw: {arguments}"))
}

impl ToolKind {
    fn wire_name(&self) -> &'static str {
        match self {
            ToolKind::WebSearch => "web_search",
            ToolKind::VisitUrl => "visit_url",
            ToolKind::RunBashCommand => "run_bash_command",
            ToolKind::ResetBashContainer => "reset_bash_container",
            ToolKind::ViewImage => "view_image",
            ToolKind::SendImageToUser => "send_image_to_user",
        }
    }

        fn definition(&self) -> ToolDefinition {
        let (description, parameters): (&'static str, Value) = match self {
            ToolKind::WebSearch => (
                "Search the web — ONE focused topic per query; search one fact at a time, never pile attributes into a single query. Snippets only, usually not enough for specifics (dates, numbers, names, quotes) — open the best result with visit_url and read it before answering. For several facts, fire several single-topic searches in the same turn (parallel is fine).",
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "A few keywords for a single focused question, e.g. \"rust async runtime comparison\" or \"Stark Frieren hair color\". Don't pile unrelated attributes into one query — search one fact at a time." }
                    },
                    "required": ["query"]
                }),
            ),
            ToolKind::VisitUrl => (
                "Read a web page in full (its readable text). The normal next step after web_search — open the best result and read it before answering anything detailed or factual; the snippet alone is rarely enough. Also works on a URL the user gave you.",
                json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "The full URL to fetch, e.g. \"https://example.com/article\" (typically taken from a web_search result)" }
                    },
                    "required": ["url"]
                }),
            ),
            ToolKind::RunBashCommand => (
                "Run a bash command in your own private Linux sandbox (persistent across calls within this conversation; has python3, pip, curl, git, and internet access). Use it to compute, write and run scripts, fetch and process data, install packages — anything a shell can do. The filesystem and installed packages persist between calls, so you can build up state. Not connected to the user's machine.",
                json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The bash command to run, e.g. \"python3 -c 'print(2**10)'\" or \"pip install requests && python3 script.py\". Multi-line scripts are fine." }
                    },
                    "required": ["command"]
                }),
            ),
            ToolKind::ResetBashContainer => (
                "Wipe your sandbox and start fresh — destroys the current Linux environment (files, installed packages, processes) and the next run_bash_command boots a clean one. Use if it's in a broken state or you want a clean slate.",
                json!({ "type": "object", "properties": {}, "required": [] }),
            ),
            ToolKind::ViewImage => (
                "Look at an image file from your bash sandbox so you can see its contents. The path points to a file in the SAME private Linux environment as run_bash_command — create, download, or generate the image there first (e.g. with matplotlib, imagemagick, or curl). The file must be a valid image (PNG, JPEG, GIF, or WebP). Only you see it — it is hidden from the user. Use it to inspect plots, screenshots, or downloaded images before deciding what to do next.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the image file inside your bash sandbox, e.g. \"/tmp/plot.png\" or \"chart.png\" (relative to the sandbox working directory)." }
                    },
                    "required": ["path"]
                }),
            ),
            ToolKind::SendImageToUser => (
                "Send an image file from your bash sandbox to the user in this chat. The path points to a file in the SAME private Linux environment as run_bash_command — create, download, or generate the image there first (e.g. with matplotlib, imagemagick, or curl). The file must be a valid image (PNG, JPEG, GIF, or WebP). It goes to the user — they see it in the chat — and you see it too (it counts as a message you sent). Use it to deliver plots, generated images, or processed pictures.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the image file inside your bash sandbox, e.g. \"/tmp/plot.png\" or \"chart.png\" (relative to the sandbox working directory)." }
                    },
                    "required": ["path"]
                }),
            ),
        };
        ToolDefinition {
            kind: "function",
            function: ToolDefFunction { name: self.wire_name(), description, parameters },
        }
    }
}

impl ToolType {
    pub fn tool_definitions() -> Vec<ToolDefinition> {
        ToolKind::iter().map(|k| k.definition()).collect()
    }

    pub fn wire_name(&self) -> &'static str {
        ToolKind::from(self).wire_name()
    }

    /// Argument values as an order-preserving map of stringified values, ready to splice into a
    /// rendered tool call. (All current tools take single string args.)
    pub fn arguments_map(&self) -> Map<String, Value> {
        let mut m = Map::new();
        match self {
            ToolType::WebSearch { query } => { m.insert("query".to_string(), json!(query)); }
            ToolType::VisitUrl { url } => { m.insert("url".to_string(), json!(url)); }
            ToolType::RunBashCommand { command } => { m.insert("command".to_string(), json!(command)); }
            ToolType::ResetBashContainer => {}
            ToolType::ViewImage { path } => { m.insert("path".to_string(), json!(path)); }
            ToolType::SendImageToUser { path } => { m.insert("path".to_string(), json!(path)); }
        }
        m
    }

            pub fn bind(name: &str, arguments: &str) -> anyhow::Result<ToolType> {
        let kind = ToolKind::iter()
            .find(|k| k.wire_name() == name)
            .ok_or_else(|| anyhow::anyhow!("model called an unknown tool: {name}"))?;

        match kind {
            ToolKind::WebSearch => Ok(ToolType::WebSearch {
                query: parse_args::<WebSearchArgs>(name, arguments)?.query,
            }),
            ToolKind::VisitUrl => Ok(ToolType::VisitUrl {
                url: parse_args::<VisitUrlArgs>(name, arguments)?.url,
            }),
            ToolKind::RunBashCommand => Ok(ToolType::RunBashCommand {
                command: parse_args::<RunBashArgs>(name, arguments)?.command,
            }),
            ToolKind::ResetBashContainer => Ok(ToolType::ResetBashContainer),
            ToolKind::ViewImage => Ok(ToolType::ViewImage {
                path: parse_args::<PathArgs>(name, arguments)?.path,
            }),
            ToolKind::SendImageToUser => Ok(ToolType::SendImageToUser {
                path: parse_args::<PathArgs>(name, arguments)?.path,
            }),
        }
    }
}
