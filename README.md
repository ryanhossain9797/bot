# A2

A2 is a Discord chatbot written in Rust. It runs its LLM in-process with `llama.cpp` (or, behind
a config switch, against an Anthropic-compatible API), calls tools over many rounds per turn, and
keeps every conversation as a persistent state machine that survives restarts.

## What it does

- **Chats in DMs and servers.** In a DM every message is for the bot. In a channel it stays quiet
  unless @mentioned, or occasionally when it has something to add; staying quiet is the literal
  reply `[EMPTY]`.
- **Sees images** attached to messages, and can send files and images back.
- **Uses tools**, up to 30 rounds per turn:

  | Tool | What it does |
  |------|--------------|
  | `web_search` | Searches the web through a self-managed SearxNG instance (snippets only) |
  | `visit_url` | Fetches a page and extracts its readable text |
  | `run_bash_command` | Runs a command in a private, per-conversation Docker sandbox |
  | `reset_bash_container` | Wipes that sandbox |
  | `read_file` | Reads a sandbox file, line-numbered and sliceable |
  | `edit_file` | Exact-string replace in a sandbox file, with concurrency checks |
  | `view_image` | Lets the model look at a sandbox image (the user does not see it) |
  | `use_skill` | Loads a how-to from [`chatbot/skills/`](chatbot/skills/) |
  | `set_reminder` | Schedules a reminder, up to about a year out |
  | `meta_no_op_extra_turn` | Gives the model another turn to keep talking |

  Sending a file or image to the user is not a tool. The model writes `[[attach_file:PATH]]` or
  `[[attach_image:PATH]]` in its reply, and the sandbox file is uploaded in its place.
- **Remembers** a 30-entry window per conversation. Once 24 entries that aren't summaries pile
  up, a smaller utility model summarizes everything but the latest 12.

## Repository

| Path | Contents |
|------|----------|
| [`chatbot/`](chatbot/) | The bot |
| [`re_framework/`](re_framework/) | The persistent state-machine framework the bot is built on |
| [`sample_framework_project/`](sample_framework_project/) | A minimal replica of the bot used to test the framework |
| [`probe/`](probe/) | Drives a synthetic, bot-shaped conversation through an Anthropic-compatible endpoint |
| [`extractor/`](extractor/) | Dev tool that dumps a GGUF's chat template and metadata |

Inside `chatbot/src/`:

| Path | Contents |
|------|----------|
| `state_machines/` | `ConversationMachine`, `MemoryManagerMachine`, `ReminderForConversationMachine` |
| `externals/` | Side effects the machines enqueue: LLM calls, tools, sending messages, summarizing |
| `llm/` | The LLM backend: Anthropic Messages API types and the remote client |
| `llm/native/` | Everything `llama.cpp`: model packs, prompt rendering, the decode loop, output parsers |
| `prompts.rs` | System prompts and sampling settings |
| `tools.rs` | Tool definitions and argument binding |
| `services/discord.rs` | Discord gateway handling |
| `configuration.rs` | Tokens and switches (gitignored, copy from `configuration.rs.template`) |

## How a turn runs

```
Discord message
      │
      ▼
ConversationMachine ── queued in `pending`
      │
      ▼
AwaitingLLMDecision ──▶ llm_external: build request → LLM backend → parse
      │
      ├── reply text ──▶ SendingMessage ──▶ Discord
      └── tool calls ──▶ RunningTools ──▶ results fed back ──▶ AwaitingLLMDecision
                                                               (until no tools, or 30 rounds)
      ▼
    Idle
```

Every state that waits on something has a deadline: 15 minutes for the LLM, 1 minute to send a
message, and a per-tool limit while tools run. When a deadline passes, the machine receives a
timeout result and moves on.

## The framework (`re_framework`)

You implement `StateMachine`: `construct`, `transition`, `schedule` and `name`, over your own
`State`, `Id`, `Action`, `Construction`, `Failure` and `Env` types. Each live entity is a Tokio
task with a mailbox, reloaded from the store when first touched.

Transitions are pure. Instead of doing I/O they return effects:

- `enqueue_action` sends a durable action to another machine. It's written to an outbox in the
  same transaction as the new state, redelivered after crashes and deduplicated at the receiver.
- `enqueue_external(future)` runs async work once the transition has committed. The future
  resolves to one of the machine's own actions, which is fed back in. It can't fail: errors
  have to come back as actions.
- `schedule(state)` returns the next timed action (a deadline or a reminder), and the runtime
  fires it when due.

The store is the source of truth: each transition commits the state (compare-and-swap on
version and generation), its outbox rows and its dedup marker together, or none of them. Turso
(SQLite) is the only store today. `store::contract` is the test suite any new store must pass.

## The LLM backend

The externals speak the Anthropic Messages API: a request has a system prompt, messages made
of text, image, `tool_use` and `tool_result` blocks, and tools with JSON schemas. The response
comes back as `thinking`, `text` and `tool_use` blocks. `configuration::llm::BACKEND` picks
what serves them:

- **`Native`** loads the model packs and runs them in-process. The request is translated to the
  pack's chat template and the raw output parsed back into blocks.
- **`Remote`** posts the request to an Anthropic-compatible `/v1/messages` endpoint
  (`base_url`, `api_key`, `api_version`, `primary_model`, `utility_model`, `timeout_secs`), all
  checked at startup. History is normalized for the API first: same-role turns merged, repeated
  `tool_use` ids renamed, orphaned `tool_result`s turned into text, unanswered `tool_use`s given
  error results, empty text dropped, and a user turn first. No local model is loaded. On this
  host the endpoint is Halogen (`http://127.0.0.1:8731`, model `halogen-qwen3.8-flash-next`),
  which serves both roles; the template has that config commented out.

Before either backend sees a request, its images are checked: each gets the media type its bytes
actually have, formats other than PNG, JPEG and GIF (WebP included, which llama.cpp can't read)
are re-encoded as JPEG, and one that doesn't decode is replaced by a short note, so a single bad attachment can't fail every later
turn.

One field isn't part of the API: the per-turn footer (time, chat setting, tool budget). Native
renders it after the conversation; remote appends it to the last user turn, so the system prompt
stays cacheable.

### Model packs

Everything model-specific lives in a pack folder under `chatbot/models/`: GGUF weights, an
optional multimodal projector, the chat template and a `manifest.toml` (sampling, context size,
thinking marker, output parser). The primary pack is
[`qwen3-8-27b`](chatbot/models/qwen3-8-27b/), and [`qwen3-4b`](chatbot/models/qwen3-4b/)
handles summaries. `MODEL_PACK_DIR` and `UTILITY_MODEL_PACK_DIR` choose which packs to load.

When the GGUF has an MTP head, generation uses MTP speculative decoding: it drafts up to
`draft_max` tokens per step and verifies them in one batch. The output is the same as standard
decoding, roughly twice as fast on Qwen3.8-27B. An optional `[speculative]` section in the
manifest tunes it (`mtp = false` turns it off). `chatbot bench [message] [image]` times both
modes on one prompt.

## Running it

### You need

- Docker. The deploy targets an AMD GPU (Vulkan) host.
- A Discord bot token, with the **Message Content** privileged intent enabled. Without it the
  bot can't read channel messages that don't mention it.
- The model packs' GGUF files in `chatbot/models/`. They're copied into the base image, not
  mounted, so use real files or hard links; `docker build` doesn't follow symlinks.

### Configure

```bash
cp chatbot/src/configuration.rs.template chatbot/src/configuration.rs
```

Fill in `DISCORD_TOKEN`, `SEARXNG_URL` and the switches. Values are compiled in, so changing one
means rebuilding.

### Build and deploy (from `chatbot/`)

```bash
just build_base      # runtime deps + model packs
just deploy_local    # build the app image and (re)start the `bot` container
```

The container uses host networking and mounts the Docker socket, so it can run the bash
sandboxes and SearxNG as sibling containers. Deploys are manual for now: run `just deploy_local`
from an up-to-date `main` checkout on the bot's host. The `deploy` workflow's self-hosted runner
is down.

### Web search

When `SEARXNG_URL` points at localhost (the default `http://localhost:8080`), the bot builds and
runs its own `bot-searxng` container (JSON output on, random secret) and restarts it whenever
it's missing or stopped. A non-localhost URL is used as-is, and that instance must have JSON
output enabled.

### Environment variables

| Variable | Default |
|----------|---------|
| `MODEL_PACK_DIR` | `./models/qwen3-8-27b` |
| `UTILITY_MODEL_PACK_DIR` | `./models/qwen3-4b` |

## Development

The crate needs `cmake`, `glslc`, `clang` and Vulkan headers to build `llama.cpp`, so build and
test in the builder image:

```bash
docker build --target builder -t chatbot-builder -f chatbot/Dockerfile .
docker run --rm -v "$PWD/chatbot/src:/app/chatbot/src:ro" -v "$PWD/chatbot/models:/app/chatbot/models:ro" \
    -w /app chatbot-builder cargo test --release --package chatbot
```

`primary_request_round_trips_through_a_live_remote` is ignored by default. It sends the golden-test
conversation through the real remote client; run it against Halogen with
`--network host -e REMOTE_LLM_URL=http://127.0.0.1:8731 -e REMOTE_LLM_KEY=… -e
REMOTE_LLM_MODEL=halogen-qwen3.8-flash-next` and `cargo test … live_remote -- --ignored`.
For a quicker look at an endpoint, `cargo run -p probe -- conversation --stream` builds on the
host.

The toolchain is pinned to `nightly-2026-10-02` because `branches` 0.4.x, pulled in by turso,
calls `core::intrinsics::abort`, which later nightlies removed
([fereidani/branches#10](https://github.com/fereidani/branches/issues/10)). Unpin once a turso
release depends on a fixed `branches`.

### No comments

The code carries no comments; names, types and this README carry the explanation. CI runs
[`uncomment`](https://crates.io/crates/uncomment) 3.10.3 on every pull request and fails if it
finds any, doc comments included:

```bash
cargo install uncomment --version 3.10.3 --locked
uncomment --check --remove-doc .
```

`uncomment --remove-doc .` strips them. A comment that truly has to stay (such as a `SAFETY:` note
on an `unsafe` block) can end with `~keep`.

## License

MIT
