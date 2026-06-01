//! Tool registry, driven by `ToolCall`'s variants via the strum-generated `ToolKind`.
//!
//! Every match over `ToolKind` below is exhaustive with one arm per variant, so adding a
//! `ToolCall` variant won't compile until it's handled here — no tool can be silently dropped
//! (compile-time, which is stronger than a runtime check). A tool that is advertised but whose
//! argument binding isn't implemented yet returns a runtime error from `bind`.
//!
//! strum gives us the *iteration*; the per-variant schema is still hand-written (Rust can't
//! reflect a variant's payload into JSON Schema). At higher tool counts, switch to
//! `GetWeather(GetWeatherArgs)` payloads + `schemars::JsonSchema` to derive it.

use serde::Deserialize;
use serde_json::{json, Value};
use strum::IntoEnumIterator;

use crate::models::user::{ToolCall, ToolKind};

/// Typed view of `get_weather`'s arguments. The model emits these as a JSON string; we deserialize
/// into this struct so a malformed/incomplete call is a recoverable error, not a panic.
#[derive(Debug, Deserialize)]
struct GetWeatherArgs {
    city: String,
}

impl ToolKind {
    /// Stable wire name for this tool — the single source used by both the advertised schema and
    /// the inverse (name → variant) lookup in `bind`.
    fn wire_name(&self) -> &'static str {
        match self {
            ToolKind::GetWeather => "get_weather",
            ToolKind::MathCalculation => "math_calculation",
            ToolKind::WebSearch => "web_search",
            ToolKind::VisitUrl => "visit_url",
        }
    }

    /// The OpenAI-style tool entry advertised to the model, or `None` if this variant isn't
    /// exposed yet (it stays wired for execution, just unadvertised). Exhaustive: every variant
    /// must declare its status here.
    fn definition(&self) -> Option<Value> {
        match self {
            ToolKind::GetWeather => Some(json!({
                "type": "function",
                "function": {
                    "name": self.wire_name(),
                    "description": "Get the current weather for a city. Use when the user asks about weather, temperature, or conditions for a specific place.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string", "description": "City name, e.g. \"Paris\" or \"London\"" }
                        },
                        "required": ["city"]
                    }
                }
            })),
            ToolKind::MathCalculation => None,
            ToolKind::WebSearch => None,
            ToolKind::VisitUrl => None,
        }
    }
}

impl ToolCall {
    /// OpenAI-style tool array advertised to the model, assembled from every advertised variant.
    pub fn tools_json() -> String {
        let entries: Vec<Value> = ToolKind::iter().filter_map(|k| k.definition()).collect();
        Value::Array(entries).to_string()
    }

    /// Bind a model-emitted tool call to a `ToolCall`.
    ///
    /// `arguments` is the raw JSON string from `tool_calls[].function.arguments`. The name is
    /// resolved against the variant list (unknown → runtime error), then bound per variant. The
    /// per-variant match is exhaustive, so a new variant must be handled here; variants without a
    /// binding yet return a runtime error rather than compiling to a silent gap.
    pub fn bind(name: &str, arguments: &str) -> anyhow::Result<ToolCall> {
        let kind = ToolKind::iter()
            .find(|k| k.wire_name() == name)
            .ok_or_else(|| anyhow::anyhow!("model called an unknown tool: {name}"))?;

        match kind {
            ToolKind::GetWeather => {
                let args: GetWeatherArgs = serde_json::from_str(arguments).map_err(|e| {
                    anyhow::anyhow!("get_weather arguments failed to bind: {e} — raw: {arguments}")
                })?;
                Ok(ToolCall::GetWeather {
                    location: args.city,
                })
            }
            ToolKind::MathCalculation | ToolKind::WebSearch | ToolKind::VisitUrl => Err(
                anyhow::anyhow!("tool '{name}' is advertised but not wired for binding yet"),
            ),
        }
    }
}
