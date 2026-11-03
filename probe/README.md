# probe

A small CLI that drives a synthetic, bot-shaped conversation through an Anthropic-compatible
`/v1/messages` endpoint (Halogen on localhost by default) and prints what comes back. It's a
scratchpad for checking a remote model before the bot's remote backend talks to it. It has no
llama.cpp dependency, so it builds on the host.

## Run it

```bash
cargo run -p probe -- conversation --stream
```

Scenarios:

- `hello`: one user message, no tools.
- `conversation` (default): an image and a question that needs a tool, fake tool results fed back
  until the model answers, then a follow-up turn in the same conversation. Every user turn ends
  with a bot-style metadata footer block, and the model's own blocks (thinking with its
  signature, text, `tool_use`) are sent back unchanged.

`--stream` uses SSE, the way Claude Code does. Without it, requests are plain JSON.
`--drop-thinking` replays the model's turns without their thinking blocks, as the bot's history does.

For each request it prints the size and the shape of the history (`user[text,image]
assistant[thinking,tool_use] …`), then the status, latency, time to first token when
streaming, every content block, `stop_reason` and `usage`.

## Settings

| Variable | Default |
|----------|---------|
| `PROBE_BASE_URL` | `http://127.0.0.1:8731` |
| `PROBE_MODEL` | `halogen-qwen3.8-flash-next` |
| `PROBE_MAX_TOKENS` | `2048` |
| `ANTHROPIC_API_KEY` | none; sent as `x-api-key` when set |
