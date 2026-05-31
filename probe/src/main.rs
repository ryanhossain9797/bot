// Minimal local probe: drive the model with its NATIVE chat template (+ a fake tool) and dump the
// full prompt, the tokens, the raw output, and the OAICompat JSON. Renders via the params path with
// `reasoning_format` set, so the parser splits reasoning_content from content. (Run: `cargo run -p probe`.)
use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, AddBos, LlamaModel, Special},
    openai::OpenAIChatTemplateParams,
    sampling::LlamaSampler,
    send_logs_to_tracing, LogOptions,
};
use std::{io::Write, num::NonZero};

const MODEL: &str = "/home/zireael9797/Repos/bot/chatbot/models/Qwen3.6-27B-Q4_K_M.gguf";
const N_CTX: u32 = 8192;
const MAX_TOKENS: usize = 1024;

// ---- hardcoded fake tool, OpenAI-compatible JSON array ----
const TOOLS: &str = r#"[{"type":"function","function":{
  "name":"get_weather",
  "description":"Get the current weather for a city",
  "parameters":{"type":"object","properties":{
    "city":{"type":"string","description":"City name, e.g. Paris"}},
    "required":["city"]}}}]"#;

// reasoning extraction format: "auto" | "deepseek" | "deepseek-legacy" | "none"
const REASONING_FORMAT: &str = "auto";

fn main() -> anyhow::Result<()> {
    send_logs_to_tracing(LogOptions::default().with_logs_enabled(false)); // silence llama.cpp/ggml logs
    let backend = LlamaBackend::init()?;
    let model = LlamaModel::load_from_file(
        &backend,
        MODEL,
        &LlamaModelParams::default().with_n_gpu_layers(999), // offload everything to GPU (Vulkan)
    )?;

    // ---- hardcode the conversation: simulate being on the 2nd question, so "there" => Paris ----
    let messages_json = serde_json::json!([
        {"role": "user", "content": "What is the capital of France"},
        {"role": "assistant", "content": "It's Paris"},
        {"role": "user", "content": "What's the weather there? Even if you call a tool, tell me what tool you called"}
    ])
    .to_string();

    // Render via the params path so we can set reasoning_format (splits <think> into reasoning_content).
    let template = model.chat_template(None)?;
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
    let rendered = model.apply_chat_template_oaicompat(&template, &params)?;
    let prompt = &rendered.prompt;

    println!("\n========== RAW PROMPT ==========\n{prompt}");

    let mut ctx = model.new_context(
        &backend,
        LlamaContextParams::default().with_n_ctx(Some(NonZero::new(N_CTX).unwrap())),
    )?;

    let tokens = model.str_to_token(prompt, AddBos::Never)?;

    let mut batch = LlamaBatch::new(N_CTX as usize, 1);
    let last = tokens.len() - 1;
    for (i, t) in tokens.iter().enumerate() {
        batch.add(*t, i as i32, &[0], i == last)?;
    }
    ctx.decode(&mut batch)?;

    // Qwen's recommended sampler (per LM Studio): temp 0.6 / top_k 20 / top_p 0.95, no repeat penalty.
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(0.6),
        LlamaSampler::top_k(20),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::dist(0),
    ]);

    println!("\n========== RAW OUTPUT ==========");
    let mut out = String::new();
    let mut n_cur = tokens.len() as i32;
    let mut last_idx = batch.n_tokens() - 1;
    for _ in 0..MAX_TOKENS {
        let tok = sampler.sample(&ctx, last_idx);
        if model.is_eog_token(tok) {
            break;
        }
        let piece = model.token_to_str(tok, Special::Tokenize)?;
        print!("{piece}");
        std::io::stdout().flush()?;
        out.push_str(&piece);
        batch.clear();
        batch.add(tok, n_cur, &[0], true)?;
        ctx.decode(&mut batch)?;
        n_cur += 1;
        last_idx = batch.n_tokens() - 1;
    }

    // The binding's parser: raw stream -> OpenAI-style JSON. With reasoning_format set above, this
    // now separates reasoning_content (the <think> block) from content and tool_calls.
    let parsed = rendered.parse_response_oaicompat(&out, false)?;
    println!("\n\n========== OAICOMPAT JSON (parse_response_oaicompat) ==========\n{parsed}");
    Ok(())
}
