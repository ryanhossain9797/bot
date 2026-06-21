use llama_cpp_2::gguf::GgufContext;
use std::path::{Path, PathBuf};

const DEFAULT_ROOT: &str = "extracted";

const T_UINT8: u32 = 0;
const T_INT8: u32 = 1;
const T_UINT16: u32 = 2;
const T_INT16: u32 = 3;
const T_UINT32: u32 = 4;
const T_INT32: u32 = 5;
const T_FLOAT32: u32 = 6;
const T_BOOL: u32 = 7;
const T_STRING: u32 = 8;
const T_ARRAY: u32 = 9;
const T_UINT64: u32 = 10;
const T_INT64: u32 = 11;
const T_FLOAT64: u32 = 12;

fn type_name(t: u32) -> &'static str {
    match t {
        T_UINT8 => "u8",
        T_INT8 => "i8",
        T_UINT16 => "u16",
        T_INT16 => "i16",
        T_UINT32 => "u32",
        T_INT32 => "i32",
        T_FLOAT32 => "f32",
        T_BOOL => "bool",
        T_STRING => "str",
        T_ARRAY => "arr",
        T_UINT64 => "u64",
        T_INT64 => "i64",
        T_FLOAT64 => "f64",
        _ => "?",
    }
}

fn scalar(g: &GgufContext, idx: i64, t: u32) -> String {
    match t {
        T_STRING => g.val_str(idx).unwrap_or("<non-utf8>").to_string(),
        T_UINT32 => g.val_u32(idx).to_string(),
        T_INT32 => g.val_i32(idx).to_string(),
        T_UINT64 => g.val_u64(idx).to_string(),
        T_ARRAY => "<array>".to_string(),
        _ => "<unread>".to_string(),
    }
}

fn str_key(g: &GgufContext, key: &str) -> Option<String> {
    let idx = g.find_key(key);
    if idx >= 0 && g.kv_type(idx) as u32 == T_STRING {
        g.val_str(idx).map(str::to_string)
    } else {
        None
    }
}

fn u32_key(g: &GgufContext, key: &str) -> Option<u32> {
    let idx = g.find_key(key);
    (idx >= 0 && g.kv_type(idx) as u32 == T_UINT32).then(|| g.val_u32(idx))
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn quant_name(ft: u32) -> String {
    match ft {
        0 => "f32".to_string(),
        1 => "f16".to_string(),
        2 => "q4_0".to_string(),
        3 => "q4_1".to_string(),
        7 => "q8_0".to_string(),
        8 => "q5_0".to_string(),
        9 => "q5_1".to_string(),
        10 => "q2_k".to_string(),
        12 => "q3_k_m".to_string(),
        15 => "q4_k_m".to_string(),
        17 => "q5_k_m".to_string(),
        18 => "q6_k".to_string(),
        n => format!("ft{n}"),
    }
}

// Unique-ish directory name from the model's embedded identity: general.name (or
// basename + size_label, else the file stem), plus the quant so two quantizations
// of the same model don't collide.
fn model_dir_name(g: &GgufContext, model_path: &str) -> String {
    let name = str_key(g, "general.name")
        .or_else(|| match (str_key(g, "general.basename"), str_key(g, "general.size_label")) {
            (Some(b), Some(s)) => Some(format!("{b} {s}")),
            _ => None,
        })
        .unwrap_or_else(|| {
            Path::new(model_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("model")
                .to_string()
        });
    let base = slug(&name);
    match u32_key(g, "general.file_type") {
        Some(ft) => format!("{base}-{}", quant_name(ft)),
        None => base,
    }
}

fn main() -> anyhow::Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let verify = raw.iter().any(|a| a == "--verify");
    let mut positional = raw.iter().filter(|a| !a.starts_with("--")).cloned();
    let model_path = positional
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: extractor <model.gguf> [out_root] [--verify]"))?;
    let root = positional.next().unwrap_or_else(|| DEFAULT_ROOT.to_string());

    let g = GgufContext::from_file(Path::new(&model_path))
        .ok_or_else(|| anyhow::anyhow!("could not open GGUF (missing or invalid): {model_path}"))?;

    let out = PathBuf::from(&root).join(model_dir_name(&g, &model_path));
    std::fs::create_dir_all(&out)?;

    let ct_idx = g.find_key("tokenizer.chat_template");
    if ct_idx >= 0 && g.kv_type(ct_idx) as u32 == T_STRING {
        if let Some(tmpl) = g.val_str(ct_idx) {
            std::fs::write(out.join("chat_template.jinja"), tmpl)?;
        }
    }

    let n = g.n_kv();
    let mut report = String::new();
    for idx in 0..n {
        let key = g.key_at(idx).unwrap_or("<?>");
        let t = g.kv_type(idx) as u32;
        let val = if key == "tokenizer.chat_template" {
            "<chat_template.jinja>".to_string()
        } else {
            let v = scalar(&g, idx, t);
            if v.chars().count() > 160 {
                format!("{}…", v.chars().take(160).collect::<String>())
            } else {
                v
            }
        };
        report.push_str(&format!("{key}\t{}\t{val}\n", type_name(t)));
    }
    std::fs::write(out.join("metadata.tsv"), &report)?;

    println!("{} keys, {} tensors -> {}", n, g.n_tensors(), out.display());

    if verify {
        verify_template(&model_path, &out)?;
    }
    Ok(())
}

// Cross-check the raw GgufContext extract against what llama.cpp's own model API returns
// (a different code path reading the same GGUF). Vocab-only load — no weights, no GPU.
fn verify_template(model_path: &str, out: &Path) -> anyhow::Result<()> {
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::LlamaModel;

    let extracted = std::fs::read_to_string(out.join("chat_template.jinja"))?;
    let backend = LlamaBackend::init()?;
    let model = LlamaModel::load_from_file(
        &backend,
        model_path,
        &LlamaModelParams::default().with_vocab_only(true),
    )?;
    let via_api = model.chat_template(None)?.to_string()?;

    if via_api == extracted {
        println!(
            "verify: OK — matches model.chat_template() byte-for-byte ({} bytes)",
            extracted.len()
        );
        Ok(())
    } else {
        anyhow::bail!(
            "verify: MISMATCH — extracted {} bytes vs llama.cpp api {} bytes",
            extracted.len(),
            via_api.len()
        )
    }
}
