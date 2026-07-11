# Models Directory — Model Packs

This directory holds **model packs**: self-contained folders, one per model, that bundle everything
model-specific. The bot loads exactly one pack at startup (chosen by the `MODEL_PACK_DIR`
environment variable), and nothing about a model is compiled into the binary — swapping the folder
and restarting is enough to change models.

## What a pack contains

```
qwen-qwen3-6-35b-a3b/
├── manifest.toml        # the knobs (see below)
├── chat_template.jinja  # the chat template the prompt is rendered from
├── <model>.gguf         # GGUF weights (referenced by manifest `model`)
└── mmproj-<...>.gguf     # multimodal projector (referenced by manifest `mmproj`)
```

The `.gguf` weight/projector files are large and are **not** checked into git (see
[.gitignore](../../.gitignore)). Download them into the pack folder before running — the
`manifest.toml` names the exact filenames it expects.

## The manifest

`manifest.toml` is the pack's contract with the loader ([chatbot/src/model_pack.rs](../src/model_pack.rs)).
The bundled Qwen pack:

```toml
model    = "Qwen3.6-35B-A3B-Q8_0.gguf"       # GGUF weights (filename within this folder)
mmproj   = "mmproj-Qwen3.6-35B-A3B-BF16.gguf" # multimodal projector
template = "chat_template.jinja"              # chat template file within this folder

[sampling]
top_k = 20
top_p = 0.95

[context]
n_ctx                 = 196608   # context window
n_batch               = 4096     # llama.cpp batch size
batch_chunk           = 2048     # tokens decoded per chunk while ingesting the prompt
max_generation_tokens = 8192     # cap on generated tokens per turn

[format]
enable_thinking       = true     # emit a reasoning block
add_generation_prompt = true     # template appends the assistant generation prompt
parser                = "qwen"   # response parser to use (resolved in roles/parsers)

[thinking]
close_marker = "</think>"        # marker that ends the reasoning block
```

The `parser` name is resolved against the parser family in
[chatbot/src/roles/parsers.rs](../src/roles/parsers.rs). A model whose output grammar matches an
existing parser is a manifest change; a genuinely new grammar means adding a `Parser` impl and a
line in `from_name`.

## Supported format

Weights and projector must be **GGUF** (the quantized format llama.cpp loads). The projector
(`mmproj`) is what gives the model its vision capability.

## Adding a new pack

1. Create a folder here, e.g. `models/my-model/`.
2. Drop the GGUF weights and mmproj projector into it.
3. Write a `manifest.toml` (copy the Qwen one and adjust filenames/knobs).
4. Add the chat template the model expects as a `.jinja` file and point `template` at it.
5. Make sure `[format] parser` names a parser the bot knows (`qwen` today), or add one.
6. Run with `MODEL_PACK_DIR=./models/my-model` (see the `Justfile`'s `run_local`, which mounts the
   pack and sets this variable).

## Where to get GGUF weights

Pre-quantized GGUF models are easiest to obtain from Hugging Face:

- Qwen: <https://huggingface.co/Qwen>
- Community quantizers: <https://huggingface.co/bartowski>, <https://huggingface.co/lmstudio-community>

```bash
# example: fetch a weight file straight into a pack folder
huggingface-cli download <repo> <file>.gguf \
  --local-dir models/qwen-qwen3-6-35b-a3b
```

Remember to fetch the matching **mmproj** projector file for multimodal support, not just the
weights.

## Troubleshooting

**Model fails to load** — confirm the `.gguf` files downloaded completely, that their filenames
match `manifest.toml` exactly, and that there's enough VRAM/RAM for the quantization you chose.

**Poor quality responses** — try a higher-quality quantization (e.g. Q6_K / Q8_0), and verify the
`chat_template.jinja` and `close_marker` actually match the model family.

**Slow performance** — use a smaller model or a lower quantization, or lower `n_ctx` /
`max_generation_tokens` in the manifest.

## Additional resources

- llama.cpp: <https://github.com/ggerganov/llama.cpp>
- GGUF spec: <https://github.com/ggerganov/ggml/blob/master/docs/gguf.md>
