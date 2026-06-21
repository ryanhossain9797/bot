# extractor

A small dev tool that reads a model's GGUF **metadata header** (no weights loaded, no GPU)
and dumps the parts we need to build our own prompt-rendering pipeline — chiefly the embedded
chat template.

It's part of the chat-harness overhaul: the plan is to render prompts ourselves (Rust-native,
minijinja) from per-model template files in git, rather than going through the model's shipped
template via llama.cpp's OpenAI-compat layer. This tool extracts the shipped template as the
starting point, and the metadata that feeds a per-model manifest.

## What it writes

For each model it creates `extracted/<model-id>/` containing:

- `chat_template.jinja` — the `tokenizer.chat_template` verbatim.
- `metadata.tsv` — every GGUF metadata key with its type and (for scalars/strings) value;
  arrays and the chat template are elided.

`<model-id>` is derived from the model's embedded identity — `general.name` (or
`general.basename` + `general.size_label`, else the file stem) — plus the quant from
`general.file_type`, e.g. `qwen-qwen3-6-35b-a3b-q8_0`. Two quants of the same model don't collide.

## Run it

```bash
cargo run -p extractor -- /path/to/model.gguf [out_root]   # out_root defaults to extracted/
```

Reads only the header via `GgufContext`, so it's instant even on a 35 GB file.
