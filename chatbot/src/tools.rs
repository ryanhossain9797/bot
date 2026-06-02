use serde::Deserialize;
use serde_json::{json, Value};
use strum::IntoEnumIterator;

use crate::models::user::{ToolCall, ToolKind};

#[derive(Debug, Deserialize)]
struct GetWeatherArgs {
    city: String,
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
}

#[derive(Debug, Deserialize)]
struct VisitUrlArgs {
    url: String,
}

fn parse_args<T: serde::de::DeserializeOwned>(name: &str, arguments: &str) -> anyhow::Result<T> {
    serde_json::from_str(arguments)
        .map_err(|e| anyhow::anyhow!("{name} arguments failed to bind: {e} — raw: {arguments}"))
}

impl ToolKind {
    fn wire_name(&self) -> &'static str {
        match self {
            ToolKind::GetWeather => "get_weather",
            ToolKind::MathCalculation => "math_calculation",
            ToolKind::WebSearch => "web_search",
            ToolKind::VisitUrl => "visit_url",
            ToolKind::RecallShortTerm => "recall_short_term",
            ToolKind::RecallLongTerm => "recall_long_term",
        }
    }

    /// OpenAI tool entry, or `None` if this variant isn't advertised yet (still executable).
    fn definition(&self) -> Option<Value> {
        match self {
            ToolKind::GetWeather => Some(json!({
                "type": "function",
                "function": {
                    "name": self.wire_name(),
                    "description": "Get the current weather for a city.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string", "description": "City name, e.g. \"Paris\" or \"London\"" }
                        },
                        "required": ["city"]
                    }
                }
            })),
            ToolKind::WebSearch => Some(json!({
                "type": "function",
                "function": {
                    "name": self.wire_name(),
                    "description": "Search the web. Returns short snippets — use visit_url to read a result in full.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "A few keywords describing what to look for, e.g. \"rust async runtime comparison\"" }
                        },
                        "required": ["query"]
                    }
                }
            })),
            ToolKind::VisitUrl => Some(json!({
                "type": "function",
                "function": {
                    "name": self.wire_name(),
                    "description": "Fetch a URL's readable text — usually a web_search result, or one the user gave you.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string", "description": "The full URL to fetch, e.g. \"https://example.com/article\" (typically taken from a web_search result)" }
                        },
                        "required": ["url"]
                    }
                }
            })),
            ToolKind::MathCalculation | ToolKind::RecallShortTerm | ToolKind::RecallLongTerm => None,
        }
    }
}

impl ToolCall {
    pub fn tools_json() -> String {
        let entries: Vec<Value> = ToolKind::iter().filter_map(|k| k.definition()).collect();
        Value::Array(entries).to_string()
    }

    pub fn wire_name(&self) -> &'static str {
        ToolKind::from(self).wire_name()
    }

    /// JSON arguments to replay this call into history — the inverse of `bind`.
    pub fn wire_arguments(&self) -> String {
        match self {
            ToolCall::GetWeather { location } => json!({ "city": location }),
            ToolCall::MathCalculation { operations } => json!({ "operations": operations }),
            ToolCall::WebSearch { query } => json!({ "query": query }),
            ToolCall::VisitUrl { url } => json!({ "url": url }),
            ToolCall::RecallShortTerm { reason } => json!({ "reason": reason }),
            ToolCall::RecallLongTerm { search_term } => json!({ "search_term": search_term }),
        }
        .to_string()
    }

    /// Bind a model-emitted call (name + raw JSON arguments) to a `ToolCall`. Unknown name or
    /// unbindable tool → runtime error; the per-variant match is exhaustive so new variants must
    /// be handled here.
    pub fn bind(name: &str, arguments: &str) -> anyhow::Result<ToolCall> {
        let kind = ToolKind::iter()
            .find(|k| k.wire_name() == name)
            .ok_or_else(|| anyhow::anyhow!("model called an unknown tool: {name}"))?;

        match kind {
            ToolKind::GetWeather => Ok(ToolCall::GetWeather {
                location: parse_args::<GetWeatherArgs>(name, arguments)?.city,
            }),
            ToolKind::WebSearch => Ok(ToolCall::WebSearch {
                query: parse_args::<WebSearchArgs>(name, arguments)?.query,
            }),
            ToolKind::VisitUrl => Ok(ToolCall::VisitUrl {
                url: parse_args::<VisitUrlArgs>(name, arguments)?.url,
            }),
            ToolKind::MathCalculation | ToolKind::RecallShortTerm | ToolKind::RecallLongTerm => {
                Err(anyhow::anyhow!("tool '{name}' is not wired for binding yet"))
            }
        }
    }
}
