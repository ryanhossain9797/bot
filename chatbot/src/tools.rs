use serde::Deserialize;
use serde_json::{json, Value};
use strum::IntoEnumIterator;

use crate::types::conversation::{ToolKind, ToolType};

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
        }
    }

        fn definition(&self) -> Option<Value> {
        match self {
            ToolKind::WebSearch => Some(json!({
                "type": "function",
                "function": {
                    "name": self.wire_name(),
                    "description": "Search the web — ONE focused topic per query; search one fact at a time, never pile attributes into a single query. Snippets only, usually not enough for specifics (dates, numbers, names, quotes) — open the best result with visit_url and read it before answering. For several facts, fire several single-topic searches in the same turn (parallel is fine).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "A few keywords for a single focused question, e.g. \"rust async runtime comparison\" or \"Stark Frieren hair color\". Don't pile unrelated attributes into one query — search one fact at a time." }
                        },
                        "required": ["query"]
                    }
                }
            })),
            ToolKind::VisitUrl => Some(json!({
                "type": "function",
                "function": {
                    "name": self.wire_name(),
                    "description": "Read a web page in full (its readable text). The normal next step after web_search — open the best result and read it before answering anything detailed or factual; the snippet alone is rarely enough. Also works on a URL the user gave you.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string", "description": "The full URL to fetch, e.g. \"https://example.com/article\" (typically taken from a web_search result)" }
                        },
                        "required": ["url"]
                    }
                }
            })),
            ToolKind::RunBashCommand => Some(json!({
                "type": "function",
                "function": {
                    "name": self.wire_name(),
                    "description": "Run a bash command in your own private Linux sandbox (persistent across calls within this conversation; has python3, pip, curl, git, and internet access). Use it to compute, write and run scripts, fetch and process data, install packages — anything a shell can do. The filesystem and installed packages persist between calls, so you can build up state. Not connected to the user's machine.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": { "type": "string", "description": "The bash command to run, e.g. \"python3 -c 'print(2**10)'\" or \"pip install requests && python3 script.py\". Multi-line scripts are fine." }
                        },
                        "required": ["command"]
                    }
                }
            })),
            ToolKind::ResetBashContainer => Some(json!({
                "type": "function",
                "function": {
                    "name": self.wire_name(),
                    "description": "Wipe your sandbox and start fresh — destroys the current Linux environment (files, installed packages, processes) and the next run_bash_command boots a clean one. Use if it's in a broken state or you want a clean slate.",
                    "parameters": { "type": "object", "properties": {}, "required": [] }
                }
            })),
        }
    }
}

impl ToolType {
    pub fn tools_json() -> String {
        let entries: Vec<Value> = ToolKind::iter().filter_map(|k| k.definition()).collect();
        Value::Array(entries).to_string()
    }

    pub fn wire_name(&self) -> &'static str {
        ToolKind::from(self).wire_name()
    }

        pub fn wire_arguments(&self) -> String {
        match self {
            ToolType::WebSearch { query } => json!({ "query": query }),
            ToolType::VisitUrl { url } => json!({ "url": url }),
            ToolType::RunBashCommand { command } => json!({ "command": command }),
            ToolType::ResetBashContainer => json!({}),
        }
        .to_string()
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
        }
    }
}
