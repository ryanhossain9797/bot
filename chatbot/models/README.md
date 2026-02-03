# Models Directory

This directory contains GGUF format language models for use with this llama.cpp-based project.

## Current Models

- `Llama-3.2-1B-Instruct-Q4_K_M.gguf` - Lightweight Llama 3.2 model (1B parameters)
- `helcyon_mercury_v3.0-Q6_K.gguf` - Default model (14B parameters)

## Supported Format

This project uses **GGUF** (GPT-Generated Unified Format) models, which are quantized models optimized for running on CPU with llama.cpp.

## Where to Get Models

### 1. Hugging Face (Recommended)

The easiest way to get GGUF models is from Hugging Face repositories that provide pre-quantized versions:

**Popular GGUF Model Repositories:**
- **Qwen Models**: https://huggingface.co/Qwen
- **Llama Models**: https://huggingface.co/meta-llama
- **Quantized Collections** (various models pre-quantized):
  - https://huggingface.co/bartowski (many models in GGUF format)
  - https://huggingface.co/TheBloke (extensive GGUF collection)
  - https://huggingface.co/lmstudio-community

**Download Example:**
```bash
# Using huggingface-cli (install with: pip install huggingface-hub)
huggingface-cli download Qwen/Qwen2.5-14B-Instruct-GGUF \
  helcyon_mercury_v3.0-Q6_K.gguf \
  --local-dir models/ \
  --local-dir-use-symlinks False

# Or using wget/curl from a direct URL
wget https://huggingface.co/Qwen/Qwen2.5-14B-Instruct-GGUF/resolve/main/helcyon_mercury_v3.0-Q6_K.gguf \
  -O models/helcyon_mercury_v3.0-Q6_K.gguf
```

### 2. Ollama

You can also export models from Ollama:
```bash
# Pull a model
ollama pull qwen2.5:14b

# Note: Ollama stores models in its own format, but you can use llama.cpp tools
# to convert if needed, or download GGUF directly from Hugging Face instead
```

## Quantization Levels Explained

GGUF models come in different quantization levels. The format is typically: `Q[bits]_[variant]`

| Quantization | Size | Quality | Speed | Recommended Use |
|--------------|------|---------|-------|-----------------|
| Q2_K | Smallest | Lowest | Fastest | Testing only |
| Q3_K_M | Small | Low | Fast | Resource-constrained |
| Q4_K_M | Medium | Good | Balanced | **Recommended** |
| Q4_K_S | Medium | Good | Balanced | Alternative to Q4_K_M |
| Q5_K_M | Large | High | Slower | Quality-focused |
| Q6_K | Larger | Very High | Slower | Near-original quality |
| Q8_0 | Largest | Highest | Slowest | Maximum quality |

**For this project, Q4_K_M is recommended** as it provides a good balance of quality and performance.

## Recommended Models

### Small Models (< 4GB RAM)
- **Llama 3.2 1B**: `Llama-3.2-1B-Instruct-Q4_K_M.gguf` (~0.7GB)
- **Qwen2.5 1.5B**: `Qwen2.5-1.5B-Instruct-Q4_K_M.gguf` (~1GB)
- **Phi-3 Mini**: `Phi-3-mini-4k-instruct-Q4_K_M.gguf` (~2.4GB)

### Medium Models (8-16GB RAM)
- **Qwen2.5 7B**: `Qwen2.5-7B-Instruct-Q4_K_M.gguf` (~4.4GB)
- **Llama 3.1 8B**: `Llama-3.1-8B-Instruct-Q4_K_M.gguf` (~4.9GB)
- **Mistral 7B**: `Mistral-7B-Instruct-v0.3-Q4_K_M.gguf` (~4.4GB)

### Large Models (16GB+ RAM)
- **Qwen2.5 14B**: `helcyon_mercury_v3.0-Q6_K.gguf` (~8.7GB) **[Default]**
- **Llama 3.1 70B**: `Llama-3.1-70B-Instruct-Q4_K_M.gguf` (~39GB)
- **Qwen2.5 32B**: `Qwen2.5-32B-Instruct-Q4_K_M.gguf` (~19GB)

## Using Different Models

### Method 1: Environment Variable
```bash
# Use a different model temporarily
MODEL_PATH=models/Llama-3.2-1B-Instruct-Q4_K_M.gguf cargo run
```

### Method 2: Change Default
Edit `src/main.rs` line 15 to change the default model:
```rust
let model_path = std::env::var("MODEL_PATH")
    .unwrap_or_else(|_| "models/YOUR-MODEL-NAME.gguf".to_string());
```

## Prompt Formats

Different models use different prompt formats. This project currently uses Qwen's ChatML format:

**Qwen/ChatML Format (Current):**
```
<|im_start|>system
You are a helpful assistant.<|im_end|>
<|im_start|>user
{prompt}<|im_end|>
<|im_start|>assistant
```

**Llama Format:**
```
<|begin_of_text|><|start_header_id|>system<|end_header_id|>
You are a helpful assistant.<|eot_id|>
<|start_header_id|>user<|end_header_id|>
{prompt}<|eot_id|>
<|start_header_id|>assistant<|end_header_id|>
```

If you use a non-Qwen model, you may need to adjust the prompt format in `src/main.rs` (lines 59-62).

## Troubleshooting

### Model fails to load
- Check file integrity: ensure the .gguf file downloaded completely
- Verify RAM: ensure you have enough memory for the model
- Check permissions: ensure the file is readable

### Poor quality responses
- Try a higher quantization (Q5_K_M or Q6_K)
- Verify you're using the correct prompt format for your model
- Increase context size in main.rs (line 28)

### Slow performance
- Use a smaller model or lower quantization
- Reduce n_threads in main.rs (line 29)
- Reduce context size (currently 2048 tokens)

## Additional Resources

- llama.cpp GitHub: https://github.com/ggerganov/llama.cpp
- GGUF Specification: https://github.com/ggerganov/ggml/blob/master/docs/gguf.md
- Model Performance Comparisons: https://huggingface.co/spaces/lmsys/chatbot-arena-leaderboard
