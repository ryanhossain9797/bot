# Bot Hive — AI Chatbot with a Local LLM

A Rust Discord chatbot powered by a **local** Large Language Model, built on a homegrown,
persistent, actor-style state-machine framework.

## Overview

- **Discord Integration**: Responds to direct messages and to @mentions in group channels
- **Local LLM**: Runs Qwen3.6-35B-A3B in-process via `llama.cpp` (Vulkan/ROCm), no external API
- **Multimodal**: Accepts image attachments and can inspect or send images back
- **Tool Calling**: Multi-round tool execution (web search, page fetch, a private bash sandbox,
  file read/edit)
- **Memory**: A 30-entry rolling window of recent conversation, persisted per conversation
- **Event-sourced actors**: Each conversation is a state machine whose state is persisted to disk
  on every transition (persist-then-effect), so it survives restarts and self-recovers

## Project Structure

```
bot/
├── chatbot/                      # Main Discord chatbot application
│   ├── src/
│   │   ├── roles/                # The render → infer → parse pipeline behind a Role trait
│   │   │   ├── primary_role.rs   #   PrimaryRole — the "Terminal Alpha Beta" persona
│   │   │   ├── local_model.rs    #   Loaded GGUF model + the llama.cpp decode loop
│   │   │   ├── render.rs         #   Prompt assembly (minijinja over the pack's chat template)
│   │   │   └── parsers/          #   Per-family output parsers (e.g. qwen.rs), picked by name
│   │   ├── externals/            # Async side effects: LLM inference, tools, message sending
│   │   ├── services/             # Platform integrations (discord.rs)
│   │   ├── state_machines/       # ConversationMachine — the conversation lifecycle
│   │   ├── types/                # Domain types (conversation, media)
│   │   ├── model_pack.rs         # Loads a "model pack" folder (weights + template + manifest)
│   │   ├── chat_format.rs        # OpenAI-ish wire shapes the chat template renders from
│   │   ├── tools.rs              # Tool definitions + argument binding
│   │   ├── configuration.rs      # Tokens / feature flags (gitignored; see .template)
│   │   └── main.rs               # Entry point
│   ├── models/                   # Model packs (GGUF weights + mmproj + template + manifest.toml)
│   ├── Dockerfile / Dockerfile.base
│   └── Justfile                  # Build & deploy automation
│
├── re_framework/                 # The state-machine framework (actors + persistence)
├── probe/                        # Dev tool: visualize raw model behavior
└── extractor/                    # Dev tool: dump a GGUF's chat template + metadata
```

## Components

### Chatbot (`chatbot/`)

- **DiscordService** ([services/discord.rs](chatbot/src/services/discord.rs)): handles gateway
  events via `serenity`, normalizes mentions to `(id) Name` form, downloads image attachments, and
  routes each message into the conversation state machine.
- **ConversationMachine** ([state_machines/](chatbot/src/state_machines/conversation_state_machine.rs)):
  the single state machine, keyed by `(Platform, channel_id)`. Drives the whole turn.
- **PrimaryRole** ([roles/](chatbot/src/roles.rs)): owns the model and the full render →
  infer → parse pipeline. A single model decides and emits its tool calls directly — there is no
  separate translation/executor agent.

#### Model packs

Everything model-specific lives in a **model pack**: a self-contained folder holding the GGUF
weights, the multimodal projector, our chat template, and a `manifest.toml` of knobs (sampling,
context size, reasoning marker, which parser to use). Swap the folder via the `MODEL_PACK_DIR`
environment variable and restart to change models — nothing model-specific is compiled in. See
[chatbot/models/qwen-qwen3-6-35b-a3b/](chatbot/models/qwen-qwen3-6-35b-a3b/).

#### Available Tools

| Tool | Description |
|------|-------------|
| `web_search` | Search the web (SearxNG backend) — snippets only |
| `visit_url` | Fetch a page and extract its readable text |
| `run_bash_command` | Run a command in a private, per-conversation Linux (Docker) sandbox |
| `reset_bash_container` | Wipe the sandbox and start fresh |
| `read_file` | Read a text file from the sandbox (line-numbered, sliceable) |
| `edit_file` | Exact-string replace in a sandbox file, with optimistic-concurrency checks |
| `view_image` | Privately inspect a sandbox image (the user does **not** see it) |
| `send_image_to_user` | Send a sandbox image into the chat (the user sees it) |

The user-facing reply itself is **not** a tool — the model emits it as its message content
directly; tool calls run alongside or after it.

### Framework (`re_framework/`)

A minimal typed actor system. You implement the `StateMachine` trait with associated types
(`State`, `Id`, `Action`, `Construction`, `Env`) and four functions (`construct`, `transition`,
`schedule`, `handle`).

- **`StateMachineHandle`**: a registry (`DashMap<id → mailbox>`); `maybe_construct` / `act` /
  `delete`. Each live entity is a `tokio` task with an mpsc mailbox; entities lazily rehydrate from
  disk on first access.
- **`Effects`**: transitions don't perform side effects directly — they *enqueue* them.
  `enqueue_action` sends a durable action to another machine (serialized into a transactional
  outbox, redelivered across crashes, deduped at the receiver); `enqueue_external(future)` runs
  an async op at-most-once and feeds its result back as an action. Effects fire only *after*
  the transition commits.
- **Persistence** (#186): a Turso (SQLite) database is the source of truth — one transaction per
  transition commits {state CAS on `version` + outbox rows + dedup marker}. A CAS conflict drops
  the in-memory actor, which rebuilds from the store on its next message.
- **`Scheduled`**: an entity can declare a timed wakeup (used here for a force-reset watchdog).

## Architecture

### Conversation flow

```
Discord message
     │
     ▼
ConversationMachine ── NewMessage ──▶ (queue into `pending`)
     │
     ▼
AwaitingLLMDecision ──▶ get_llm_decision (render prompt, run inference, parse)
     │
     ▼
LLMDecisionResult
     ├── reply text ─▶ SendingMessage ─▶ Discord
     └── tool calls ─▶ RunningTools ─▶ (execute concurrently)
                             │
                             ▼
                   feed results back ─▶ AwaitingLLMDecision  (up to MAX_TOOL_ROUNDS = 10)
                             │
                             ▼
                           Idle
```

The state machine persists after each step and schedules a 10-minute `ForceReset` watchdog for any
non-idle state, so a stuck conversation self-recovers.

### Roles: render → infer → parse

The `Role` trait is deliberately location-agnostic — `generate` takes a prompt and returns text,
with no mention of a backend. `PrimaryRole` holds identity (system prompt, temperature, thinking
policy); the `LocalModel` under it holds the model's own facts (template, sampling, reasoning
marker, parser) loaded from the pack. A future remote role could satisfy the same contract with an
HTTP call.

## Building

### Prerequisites

- Docker (with a GPU runtime; the Justfile targets an AMD/ROCm host)
- A Discord bot token
- A SearxNG instance for `web_search` (JSON format enabled)
- A model pack under `chatbot/models/` (GGUF weights + mmproj + template + `manifest.toml`)

### Configuration

1. Copy `chatbot/src/configuration.rs.template` to `chatbot/src/configuration.rs`
2. Fill in your `DISCORD_TOKEN`, `SEARXNG_URL`, and feature flags
3. Rebuild the image

### Build & deploy (from `chatbot/`)

```bash
just build_base      # base image: Rust toolchain + llama.cpp
just deploy_local    # build the app image and (re)start the `bot` container
```

`run_local` mounts the model pack read-only and sets `MODEL_PACK_DIR` accordingly. The bot runs
with host networking and mounts the Docker socket so it can spin up per-conversation bash sandboxes.

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `MODEL_PACK_DIR` | Path to the model pack folder | `./models/qwen-qwen3-6-35b-a3b` |
| `RUST_LOG` | Log level | `info` |

## Dependencies

- **llama-cpp-2**: local LLM inference with multimodal (`mtmd`) support
- **serenity**: Discord client
- **tokio**: async runtime
- **minijinja**: chat-template rendering
- **dashmap**: the framework's entity registry

## License

MIT
