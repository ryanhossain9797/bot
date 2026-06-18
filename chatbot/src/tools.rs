use serde::Deserialize;
use serde_json::{json, Value};
use strum::IntoEnumIterator;

use crate::types::conversation::{ToolKind, ToolType};

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
        }
    }

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
            ToolKind::MathCalculation => None,
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
            ToolType::GetWeather { location } => json!({ "city": location }),
            ToolType::MathCalculation { operations } => json!({ "operations": operations }),
            ToolType::WebSearch { query } => json!({ "query": query }),
            ToolType::VisitUrl { url } => json!({ "url": url }),
        }
        .to_string()
    }

            pub fn bind(name: &str, arguments: &str) -> anyhow::Result<ToolType> {
        let kind = ToolKind::iter()
            .find(|k| k.wire_name() == name)
            .ok_or_else(|| anyhow::anyhow!("model called an unknown tool: {name}"))?;

        match kind {
            ToolKind::GetWeather => Ok(ToolType::GetWeather {
                location: parse_args::<GetWeatherArgs>(name, arguments)?.city,
            }),
            ToolKind::WebSearch => Ok(ToolType::WebSearch {
                query: parse_args::<WebSearchArgs>(name, arguments)?.query,
            }),
            ToolKind::VisitUrl => Ok(ToolType::VisitUrl {
                url: parse_args::<VisitUrlArgs>(name, arguments)?.url,
            }),
            ToolKind::MathCalculation => {
                Err(anyhow::anyhow!("tool '{name}' is not wired for binding yet"))
            }
        }
    }
}
