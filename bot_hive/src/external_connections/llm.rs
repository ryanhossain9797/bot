use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use std::num::NonZeroU32;

pub fn prepare_llm<'a>() -> anyhow::Result<(LlamaModel, LlamaBackend)> {
    let model_path = std::env::var("MODEL_PATH")
        .unwrap_or_else(|_| "models/Qwen2.5-14B-Instruct-Q4_K_M.gguf".to_string());

    println!("Loading model from: {}", model_path);

    // Initialize the llama.cpp backend
    let backend = LlamaBackend::init()?;

    // Load the model with default parameters
    let model_params = LlamaModelParams::default();
    let model = LlamaModel::load_from_file(&backend, &model_path, &model_params)?;

    Ok((model, backend))
}
