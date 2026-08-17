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

## The chat template

`chat_template.jinja` is **not** the model's raw template — it's the model's **native** template
(the one embedded in the GGUF as `tokenizer.chat_template`) with a small set of **local
customizations** applied on top. The render pipeline
([chatbot/src/roles/render.rs](../src/roles/render.rs)) feeds it `messages`, `tools`, `footer`,
`add_generation_prompt`, and `enable_thinking`, and pre-serializes `tools`/tool-call args to strings
before handing them over.

### Current customizations

Relative to the stock Qwen3 native template, ours differ in four places:

1. **tools loop** — drop the `| tojson` filter (`{{- tool }}`): the render pipeline already hands the
   template serialized tool strings.
2. **tool-call args** — drop the `args_value | tojson | safe` coercion, for the same reason.
3. **compaction anchor** — replace the native `raise_exception('No user query found…')` with
   *anchor-to-oldest + a 10-message cap*, so a conversation whose user turn was evicted by compaction
   still renders instead of throwing (which silently drops the bot's turn).
4. **footer block** — append a `{% if footer %}<|im_start|>system…<|im_end|>{% endif %}` block before
   the generation prompt (the "SYSTEM GENERATED CONVERSATION METADATA FOOTER"). Nothing else consumes
   the `footer` variable.

### Porting the customizations to a new model

A new model ships its **own** native template, which may differ from the current pack's (newer Qwen
templates add e.g. a `reasoning_effort` block). Don't reuse the old pack's template verbatim, and
don't take the new native verbatim — **port our customizations onto the new model's native
template**. Diffing is the tool for both halves.

Extract a model's native template from its GGUF (metadata only — no weights loaded):

```bash
python3 - <<'PY'   # pip install gguf
from gguf import GGUFReader
r = GGUFReader("models/<pack>/<model>.gguf")
f = r.get_field("tokenizer.chat_template")
open("native.jinja", "w").write(bytes(f.parts[f.data[-1]]).decode())
PY
```

1. **Identify** our customizations authoritatively: extract the **current** pack's native template and
   `diff native.jinja models/<current-pack>/chat_template.jinja`. That diff *is* the customization
   list — don't work from memory (e.g. the `</think>` split fallback and the `preserve_thinking`
   default are *native* differences between model versions, not ours).
2. **Apply** exactly those diff hunks to the **new** model's extracted native template, and save it as
   the new pack's `chat_template.jinja`.
3. **Verify** it diffs back to only the intended customizations
   (`diff new-native.jinja models/<new-pack>/chat_template.jinja`) and that it renders:
   `cargo test -p chatbot roles::render`. That test `include_str!`s `PRIMARY_TEMPLATE` in
   [render.rs](../src/roles/render.rs) — repoint it at the new pack first. It resolves at **build
   time**, so the `.jinja` must be committed (only `*.gguf` is gitignored).

## Supported format

Weights and projector must be **GGUF** (the quantized format llama.cpp loads). The projector
(`mmproj`) is what gives the model its vision capability.

## Adding a new pack

1. Create a folder here, e.g. `models/my-model/`.
2. Drop the GGUF weights and mmproj projector into it.
3. Write a `manifest.toml` (copy the Qwen one and adjust filenames/knobs).
4. Build `chat_template.jinja` from the model's native template plus our local customizations — see
   [The chat template](#the-chat-template) — and point `template` at it.
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
