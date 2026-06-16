use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{params::LlamaModelParams, LlamaModel},
    mtmd::{MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText},
    sampling::LlamaSampler,
    send_logs_to_tracing, LogOptions,
};
use std::{io::Write, num::NonZero};

const MODEL: &str = "/home/zireael9797/Repos/bot/chatbot/models/Qwen3.6-35B-A3B-Q8_0.gguf";
const MMPROJ: &str = "/home/zireael9797/Repos/bot/chatbot/models/mmproj-Qwen3.6-35B-A3B-BF16.gguf";
const N_CTX: u32 = 8192;
const N_BATCH: i32 = 512;
const MAX_TOKENS: usize = 512;

fn make_image() -> (u32, u32, Vec<u8>) {
    let (w, h) = (512u32, 512u32);
    let (cx, cy, r) = (256.0f32, 256.0f32, 180.0f32);
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= r * r {
                data.extend_from_slice(&[220, 20, 20]);
            } else {
                data.extend_from_slice(&[255, 255, 255]);
            }
        }
    }
    (w, h, data)
}

fn main() -> anyhow::Result<()> {
    send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
    let backend = LlamaBackend::init()?;
    let model = LlamaModel::load_from_file(
        &backend,
        MODEL,
        &LlamaModelParams::default().with_n_gpu_layers(999),
    )?;
    println!("model loaded");

    let mtmd = MtmdContext::init_from_file(MMPROJ, &model, &MtmdContextParams::default())?;
    println!("mtmd loaded — support_vision={} support_audio={}", mtmd.support_vision(), mtmd.support_audio());
    anyhow::ensure!(mtmd.support_vision(), "mmproj does not advertise vision support");

    let (w, h, rgb) = make_image();
    let bitmap = MtmdBitmap::from_image_data(w, h, &rgb)?;
    println!("synthetic image: {}x{} red circle on white", bitmap.nx(), bitmap.ny());

    let prompt = "<|im_start|>user\n<__media__>\nWhat is the main shape and color in this image? Answer in one short sentence.<|im_end|>\n<|im_start|>assistant\n";
    let chunks = mtmd.tokenize(
        MtmdInputText {
            text: prompt.to_string(),
            add_special: true,
            parse_special: true,
        },
        &[&bitmap],
    )?;
    println!("tokenized: {} chunks, {} tokens", chunks.len(), chunks.total_tokens());

    let mut ctx = model.new_context(
        &backend,
        LlamaContextParams::default().with_n_ctx(Some(NonZero::new(N_CTX).unwrap())),
    )?;

    let n_past = chunks.eval_chunks(&mtmd, &ctx, 0, 0, N_BATCH, true)?;
    println!("\n<<< image + prompt evaluated (n_past={n_past}) >>>\n");

    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(0.6),
        LlamaSampler::top_k(20),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::dist(0),
    ]);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut out = String::new();
    let mut n_cur = n_past;
    let mut tok = sampler.sample(&ctx, -1);
    let mut batch = LlamaBatch::new(N_CTX as usize, 1);
    for _ in 0..MAX_TOKENS {
        if model.is_eog_token(tok) {
            break;
        }
        let piece = model.token_to_piece(tok, &mut decoder, true, None)?;
        print!("{piece}");
        std::io::stdout().flush()?;
        out.push_str(&piece);
        batch.clear();
        batch.add(tok, n_cur, &[0], true)?;
        ctx.decode(&mut batch)?;
        n_cur += 1;
        tok = sampler.sample(&ctx, batch.n_tokens() - 1);
    }

    println!("\n\n--- check ---");
    let low = out.to_lowercase();
    anyhow::ensure!(low.contains("red"), "expected 'red' in description, got: {out}");
    println!("✅ vision works: model saw the image and described it (mentions 'red')");
    Ok(())
}
