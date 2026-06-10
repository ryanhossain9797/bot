use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaChatTemplate, LlamaModel},
    openai::OpenAIChatTemplateParams,
    sampling::LlamaSampler,
    send_logs_to_tracing, LogOptions,
};
use std::{io::Write, num::NonZero};

const MODEL: &str = "/home/zireael9797/Repos/bot/chatbot/models/Qwen3.6-27B-Q4_K_M.gguf";
const N_CTX: u32 = 8192;
const MAX_TOKENS: usize = 1024;
const REASONING_FORMAT: &str = "auto"; 

const TOOLS: &str = r#"[{"type":"function","function":{
  "name":"get_weather",
  "description":"Get the current weather for a city",
  "parameters":{"type":"object","properties":{
    "city":{"type":"string","description":"City name, e.g. Paris"}},
    "required":["city"]}}}]"#;

struct Probe {
    backend: LlamaBackend,
    model: LlamaModel,
    template: LlamaChatTemplate,
}

impl Probe {
    fn load() -> anyhow::Result<Self> {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
        let backend = LlamaBackend::init()?;
        let model = LlamaModel::load_from_file(
            &backend,
            MODEL,
            &LlamaModelParams::default().with_n_gpu_layers(999),
        )?;
        let template = model.chat_template(None)?;
        Ok(Self { backend, model, template })
    }

        fn respond(&self, history: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let messages_json = history.to_string();
        let params = OpenAIChatTemplateParams {
            messages_json: &messages_json,
            tools_json: Some(TOOLS),
            tool_choice: None,
            json_schema: None,
            grammar: None,
            reasoning_format: Some(REASONING_FORMAT),
            chat_template_kwargs: None,
            add_generation_prompt: true,
            use_jinja: true,
            parallel_tool_calls: false,
            enable_thinking: true,
            add_bos: false,
            add_eos: false,
            parse_tool_calls: true,
        };
        let rendered = self.model.apply_chat_template_oaicompat(&self.template, &params)?;
        print!("{}\n<Prompt / Output>\n\n", rendered.prompt);
        std::io::stdout().flush()?;
        let raw = self.generate(&rendered.prompt)?;
        let parsed = rendered.parse_response_oaicompat(&raw, false)?;
        Ok(serde_json::from_str(&parsed)?)
    }

        fn generate(&self, prompt: &str) -> anyhow::Result<String> {
        let mut ctx = self.model.new_context(
            &self.backend,
            LlamaContextParams::default().with_n_ctx(Some(NonZero::new(N_CTX).unwrap())),
        )?;
        let tokens = self.model.str_to_token(prompt, AddBos::Never)?;
        let mut batch = LlamaBatch::new(N_CTX as usize, 1);
        let last = tokens.len() - 1;
        for (i, t) in tokens.iter().enumerate() {
            batch.add(*t, i as i32, &[0], i == last)?;
        }
        ctx.decode(&mut batch)?;

        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::temp(0.6),
            LlamaSampler::top_k(20),
            LlamaSampler::top_p(0.95, 1),
            LlamaSampler::dist(0),
        ]);

        let mut decoder = encoding_rs::UTF_8.new_decoder();

        let mut out = String::new();
        let mut n_cur = tokens.len() as i32;
        let mut last_idx = batch.n_tokens() - 1;
        for _ in 0..MAX_TOKENS {
            let tok = sampler.sample(&ctx, last_idx);
            if self.model.is_eog_token(tok) {
                break;
            }
            let piece = self.model.token_to_piece(tok, &mut decoder, true, None)?;
            print!("{piece}");
            std::io::stdout().flush()?;
            out.push_str(&piece);
            batch.clear();
            batch.add(tok, n_cur, &[0], true)?;
            ctx.decode(&mut batch)?;
            n_cur += 1;
            last_idx = batch.n_tokens() - 1;
        }
        Ok(out)
    }
}

fn push(history: &mut serde_json::Value, msg: serde_json::Value) {
    history.as_array_mut().expect("history is an array").push(msg);
}

fn for_history(mut assistant: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = assistant.as_object_mut() {
        obj.remove("reasoning_content");
        if let Some(c) = obj.get("content").and_then(|v| v.as_str()) {
            let cleaned = strip_think(c);
            obj.insert("content".to_string(), serde_json::Value::String(cleaned));
        }
    }
    assistant
}

fn strip_think(s: &str) -> String {
    match (s.find("<think>"), s.find("</think>")) {
        (Some(a), Some(b)) if b >= a => {
            let end = b + "</think>".len();
            format!("{}{}", &s[..a], &s[end..]).trim().to_string()
        }
        (None, Some(b)) => s[b + "</think>".len()..].trim().to_string(),
        _ => s.trim().to_string(),
    }
}

fn content_lower(msg: &serde_json::Value) -> String {
    msg.get("content").and_then(|v| v.as_str()).unwrap_or("").to_lowercase()
}

fn main() -> anyhow::Result<()> {
    let probe = Probe::load()?;

    let mut history = serde_json::json!([
        {"role": "user", "content": "What's the capital of France"}
    ]);

    println!("\n################ TURN 1 ################\n");
    let r1 = probe.respond(&history)?;
    println!("\n--- parsed ---\n{r1}");
    assert!(content_lower(&r1).contains("paris"), "TURN 1: expected 'paris', got: {r1}");
    println!("\n✓ turn 1 response contains \"paris\"");
    push(&mut history, for_history(r1));

    println!("\n################ TURN 2 ################\n");
    push(&mut history, serde_json::json!(
        {"role": "user", "content": "What's the temparature there? say it like \"20 degree celsius\""}
    ));
    let r2 = probe.respond(&history)?;
    println!("\n--- parsed ---\n{r2}");
    let calls = r2.get("tool_calls").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let called_weather = calls
        .iter()
        .any(|c| c.pointer("/function/name").and_then(|n| n.as_str()) == Some("get_weather"));
    assert!(called_weather, "TURN 2: expected a get_weather tool call, got: {r2}");
    println!("\n✓ turn 2 called the get_weather tool");
    let tool_id = calls[0].get("id").and_then(|v| v.as_str()).unwrap_or("call_0").to_string();
    push(&mut history, for_history(r2));

    push(&mut history, serde_json::json!(
        {"role": "tool", "tool_call_id": tool_id, "content": "{\"temperature_celsius\": 25}"}
    ));

    println!("\n################ TURN 3 ################\n");
    let r3 = probe.respond(&history)?;
    println!("\n--- parsed ---\n{r3}");
    assert!(content_lower(&r3).contains("degree celsius"), "TURN 3: expected 'degree celsius', got: {r3}");
    println!("\n✓ turn 3 response contains \"degree celsius\"");

    println!("\n✅ all 3 turns passed");
    Ok(())
}
