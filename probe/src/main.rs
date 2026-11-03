use std::io::Write;
use std::time::Instant;

use anyhow::{Context, bail};
use base64::Engine;
use serde_json::{Value, json};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8731";
const DEFAULT_MODEL: &str = "halogen-qwen3.8-flash-next";
const DEFAULT_MAX_TOKENS: u32 = 2048;
const MAX_TOOL_ROUNDS: usize = 4;
const PREVIEW_CHARS: usize = 400;

const SYSTEM_PROMPT: &str = "You are A2, a helpful assistant in a Discord chat. Call tools when they \
    help; answer once you have enough. Each user turn may end with a metadata footer: read it, never \
    answer it.";

struct Config {
    base_url: String,
    model: String,
    api_key: Option<String>,
    max_tokens: u32,
    stream: bool,
    scenario: String,
}

impl Config {
    fn load() -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Config {
            base_url: env("PROBE_BASE_URL").unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model: env("PROBE_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            api_key: env("ANTHROPIC_API_KEY"),
            max_tokens: env("PROBE_MAX_TOKENS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_MAX_TOKENS),
            stream: args.iter().any(|a| a == "--stream"),
            scenario: args
                .iter()
                .find(|a| !a.starts_with("--"))
                .cloned()
                .unwrap_or_else(|| "conversation".to_string()),
        }
    }
}

struct Reply {
    content: Vec<Value>,
    stop_reason: Option<String>,
    usage: Value,
}

impl Reply {
    fn tool_uses(&self) -> Vec<(String, String, Value)> {
        self.content
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| {
                (
                    b["id"].as_str().unwrap_or_default().to_string(),
                    b["name"].as_str().unwrap_or_default().to_string(),
                    b["input"].clone(),
                )
            })
            .collect()
    }
}

fn red_circle_png() -> anyhow::Result<Vec<u8>> {
    let image = image::RgbImage::from_fn(512, 512, |x, y| {
        let (dx, dy) = (x as f32 - 256.0, y as f32 - 256.0);
        if dx * dx + dy * dy <= 180.0 * 180.0 {
            image::Rgb([220, 20, 20])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png)?;
    Ok(png.into_inner())
}

fn image_block(png: &[u8]) -> Value {
    json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": "image/png",
            "data": base64::engine::general_purpose::STANDARD.encode(png),
        }
    })
}

fn text(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn tools() -> Value {
    json!([
        {
            "name": "web_search",
            "description": "Search the web. One focused topic per query. Returns snippets.",
            "input_schema": {
                "type": "object",
                "properties": {"query": {"type": "string", "description": "A few keywords"}},
                "required": ["query"]
            }
        },
        {
            "name": "view_image",
            "description": "Privately look at an image file in your sandbox.",
            "input_schema": {
                "type": "object",
                "properties": {"path": {"type": "string", "description": "Path to the image"}},
                "required": ["path"]
            }
        }
    ])
}

fn footer(round: usize) -> Value {
    text(&format!(
        "=== SYSTEM GENERATED CONVERSATION METADATA FOOTER ===\n\
         Your username in this conversation: (7) A2.\n\
         Setting: DIRECT MESSAGE (one-to-one with the user).\n\
         Tool budget: {round}/{MAX_TOOL_ROUNDS} used."
    ))
}

fn fake_tool_result(id: &str, name: &str, input: &Value, png: &[u8]) -> Value {
    let content = match name {
        "web_search" => vec![text(&format!(
            "Results for {}:\n1. Circle - Wikipedia: a circle is the set of all points in a plane \
             at a fixed distance (the radius) from a centre. Its area is pi times the radius squared.\n\
             2. Circle (geometry) - Britannica: the ratio of circumference to diameter is pi.",
            input["query"]
        ))],
        "view_image" => vec![text("Here is the image:"), image_block(png)],
        other => vec![text(&format!("Unknown tool {other}"))],
    };
    json!({"type": "tool_result", "tool_use_id": id, "content": content})
}

fn preview(s: &str) -> String {
    let one_line = s.replace('\n', " ⏎ ");
    if one_line.chars().count() > PREVIEW_CHARS {
        format!("{}…", one_line.chars().take(PREVIEW_CHARS).collect::<String>())
    } else {
        one_line
    }
}

fn describe_request(messages: &[Value]) -> String {
    messages
        .iter()
        .map(|m| {
            let kinds: Vec<&str> = m["content"]
                .as_array()
                .map(|c| c.iter().filter_map(|b| b["type"].as_str()).collect())
                .unwrap_or_default();
            format!("{}[{}]", m["role"].as_str().unwrap_or("?"), kinds.join(","))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_reply(reply: &Reply) {
    for block in &reply.content {
        match block["type"].as_str() {
            Some("thinking") => println!(
                "  thinking ({} chars, signature {}): {}",
                block["thinking"].as_str().unwrap_or_default().len(),
                if block["signature"].as_str().unwrap_or_default().is_empty() {
                    "none"
                } else {
                    "present"
                },
                preview(block["thinking"].as_str().unwrap_or_default())
            ),
            Some("text") => println!("  text: {}", preview(block["text"].as_str().unwrap_or_default())),
            Some("tool_use") => println!(
                "  tool_use {} {}({})",
                block["id"].as_str().unwrap_or_default(),
                block["name"].as_str().unwrap_or_default(),
                block["input"]
            ),
            other => println!("  {other:?}: {}", preview(&block.to_string())),
        }
    }
    println!("  stop_reason: {:?}  usage: {}", reply.stop_reason, reply.usage);
}

struct Client {
    http: reqwest::Client,
    config: Config,
}

impl Client {
    async fn send(&self, messages: &[Value]) -> anyhow::Result<Reply> {
        let mut body = json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "system": SYSTEM_PROMPT,
            "messages": messages,
            "tools": tools(),
        });
        if self.config.stream {
            body["stream"] = json!(true);
        }
        let payload = serde_json::to_vec(&body)?;
        println!(
            "→ POST /v1/messages ({} bytes, stream={}) {}",
            payload.len(),
            self.config.stream,
            describe_request(messages)
        );

        let mut request = self
            .http
            .post(format!("{}/v1/messages", self.config.base_url.trim_end_matches('/')))
            .header("content-type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .body(payload);
        if let Some(key) = &self.config.api_key {
            request = request.header("x-api-key", key);
        }

        let started = Instant::now();
        let response = request.send().await.context("request failed")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("HTTP {status} after {:.1}s: {}", started.elapsed().as_secs_f32(), preview(&body));
        }

        let reply = if self.config.stream {
            read_stream(response, started).await?
        } else {
            let value: Value = response.json().await.context("response was not JSON")?;
            Reply {
                content: value["content"].as_array().cloned().unwrap_or_default(),
                stop_reason: value["stop_reason"].as_str().map(str::to_string),
                usage: value["usage"].clone(),
            }
        };
        println!("← HTTP {status} in {:.1}s", started.elapsed().as_secs_f32());
        print_reply(&reply);
        Ok(reply)
    }
}

async fn read_stream(mut response: reqwest::Response, started: Instant) -> anyhow::Result<Reply> {
    let mut buffer = String::new();
    let mut blocks: Vec<Value> = Vec::new();
    let mut partial_json: Vec<String> = Vec::new();
    let mut stop_reason = None;
    let mut usage = json!({});
    let mut first_token = None;

    while let Some(chunk) = response.chunk().await? {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let event: String = buffer.drain(..end + 2).collect();
            let Some(data) = event.lines().find_map(|l| l.strip_prefix("data:")) else {
                continue;
            };
            let data: Value = serde_json::from_str(data.trim())
                .with_context(|| format!("bad SSE data: {}", preview(data)))?;
            let index = data["index"].as_u64().unwrap_or(0) as usize;
            match data["type"].as_str() {
                Some("message_start") => usage = data["message"]["usage"].clone(),
                Some("content_block_start") => {
                    while blocks.len() <= index {
                        blocks.push(Value::Null);
                        partial_json.push(String::new());
                    }
                    blocks[index] = data["content_block"].clone();
                }
                Some("content_block_delta") => {
                    first_token.get_or_insert_with(|| started.elapsed());
                    let delta = &data["delta"];
                    let block = &mut blocks[index];
                    match delta["type"].as_str() {
                        Some("text_delta") => {
                            let piece = delta["text"].as_str().unwrap_or_default();
                            print!("{piece}");
                            std::io::stdout().flush().ok();
                            let joined = format!("{}{piece}", block["text"].as_str().unwrap_or_default());
                            block["text"] = json!(joined);
                        }
                        Some("thinking_delta") => {
                            let joined = format!(
                                "{}{}",
                                block["thinking"].as_str().unwrap_or_default(),
                                delta["thinking"].as_str().unwrap_or_default()
                            );
                            block["thinking"] = json!(joined);
                        }
                        Some("signature_delta") => block["signature"] = delta["signature"].clone(),
                        Some("input_json_delta") => partial_json[index]
                            .push_str(delta["partial_json"].as_str().unwrap_or_default()),
                        other => println!("\n  [unhandled delta {other:?}]"),
                    }
                }
                Some("content_block_stop") => {
                    if blocks[index]["type"] == "tool_use" && !partial_json[index].is_empty() {
                        blocks[index]["input"] = serde_json::from_str(&partial_json[index])
                            .with_context(|| format!("tool input was not JSON: {}", partial_json[index]))?;
                    }
                }
                Some("message_delta") => {
                    stop_reason = data["delta"]["stop_reason"].as_str().map(str::to_string);
                    if let Some(map) = data["usage"].as_object() {
                        for (k, v) in map {
                            usage[k] = v.clone();
                        }
                    }
                }
                Some("error") => bail!("stream error event: {}", data),
                _ => {}
            }
        }
    }
    println!();
    if let Some(t) = first_token {
        println!("  first token after {:.1}s", t.as_secs_f32());
    }
    Ok(Reply {
        content: blocks,
        stop_reason,
        usage,
    })
}

async fn run_turn(client: &Client, messages: &mut Vec<Value>, png: &[u8]) -> anyhow::Result<()> {
    for round in 0..=MAX_TOOL_ROUNDS {
        let reply = client.send(messages).await?;
        let calls = reply.tool_uses();
        messages.push(json!({"role": "assistant", "content": reply.content}));
        if calls.is_empty() {
            return Ok(());
        }
        let mut results: Vec<Value> = calls
            .iter()
            .map(|(id, name, input)| fake_tool_result(id, name, input, png))
            .collect();
        results.push(footer(round + 1));
        messages.push(json!({"role": "user", "content": results}));
    }
    bail!("model was still calling tools after {MAX_TOOL_ROUNDS} rounds")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load();
    println!(
        "probe: {} model={} scenario={} stream={} api_key={}",
        config.base_url,
        config.model,
        config.scenario,
        config.stream,
        if config.api_key.is_some() { "set" } else { "none" }
    );
    let client = Client {
        http: reqwest::Client::new(),
        config,
    };
    let png = red_circle_png()?;

    match client.config.scenario.as_str() {
        "hello" => {
            client
                .send(&[json!({"role": "user", "content": [text("Say hi in three words.")]})])
                .await?;
        }
        "conversation" => {
            let mut messages = vec![json!({"role": "user", "content": [
                text("(42) Alice: here's a picture"),
                image_block(&png),
                text("What shape and colour is this? Then look up what that shape is called in \
                      geometry and tell me one fact about it."),
                footer(0),
            ]})];
            println!("\n=== turn 1: image + question that needs a tool ===");
            run_turn(&client, &mut messages, &png).await?;

            messages.push(json!({"role": "user", "content": [
                text("(42) Alice: [Followup] thanks! now say that fact in French"),
                footer(0),
            ]}));
            println!("\n=== turn 2: follow-up in the same conversation ===");
            run_turn(&client, &mut messages, &png).await?;
        }
        other => bail!("unknown scenario {other:?}; use hello or conversation"),
    }
    println!("\nprobe: done");
    Ok(())
}
