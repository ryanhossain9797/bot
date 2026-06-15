# probe

A tiny, throwaway CLI for poking at the local model and **visualizing raw behavior**. It's
deliberately minimal (one `main.rs`, everything hardcoded) — edit the constants/conversation,
rebuild, run. Not part of the bot; it's a developer scratchpad for the chat-harness overhaul.

## What it does

Loads the GGUF onto the GPU, drives it with the model's **native chat template** (+ a hardcoded
fake tool), and prints four things:

1. **`RAW PROMPT`** — the full rendered ChatML string sent to the model (tools injected).
2. **`RAW OUTPUT`** — streamed live as the model generates, stops on `<|im_end|>` (EOS).
3. **`OAICOMPAT JSON`** — `parse_response_oaicompat` output: the binding's structured parse with
   `reasoning_content` / `content` / `tool_calls` already separated.

(It renders via `apply_chat_template_oaicompat` with `reasoning_format` set, so `<think>` lands in
`reasoning_content` instead of leaking into `content`.)

## Run it

```bash
cargo run -p probe          # or: ./target/debug/probe  (release: cargo run -p probe --release)
```

Edit the hardcoded bits at the top of [`src/main.rs`](src/main.rs):
- `MODEL` — path to the GGUF (defaults to the chatbot's Qwen3.6-35B-A3B).
- `TOOLS` — the fake tool definition (OpenAI-compatible JSON array).
- `REASONING_FORMAT` — `"auto"` | `"deepseek"` | `"deepseek-legacy"` | `"none"`.
- the `messages_json` conversation.

## Build dependencies (local)

The `vulkan` feature compiles llama.cpp from source, so the host needs (Debian/Ubuntu):

```bash
sudo apt-get install -y glslc libvulkan-dev libclang-dev
```

- `glslc` — compiles the Vulkan shaders.
- `libvulkan-dev` — Vulkan headers + loader dev symlink (CMake `FindVulkan`).
- `libclang-dev` — bindgen's clang builtin headers (else `stdbool.h not found`).

(`cmake`, a C/C++ compiler, and the Vulkan driver are also required but usually already present.)
GPU offload is forced via `with_n_gpu_layers(999)`.

> If a build fails on stale CMake state (`gmake: Makefile: No such file`), run
> `cargo clean -p llama-cpp-sys-2` and rebuild.
