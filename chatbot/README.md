# Bot Hive - AI Chatbot with Local LLM

## Overview

Bot Hive is a Discord chatbot powered by a local Large Language Model (LLM) that provides conversational AI capabilities with tool calling support. It uses a type-safe state machine framework (`framework`) to manage user interactions through distinct states and transitions.

The bot uses **Qwen3.6-27B** (quantized Q4_K_M) running locally via `llama-cpp-2`, enabling private, offline AI conversations without relying on external APIs.

## Features

- **Local LLM Integration**: Uses Qwen3.6-27B running locally via llama.cpp.
- **Single Primary Agent**: One model emits the structured tool decision directly on its `output:` line (grammar-constrained) — no separate translation/executor agent.
- **Tool Calling**: Supports multi-turn tool execution (e.g., weather lookup).
- **Conversation Context**: Maintains conversation summaries for multi-turn conversations.
- **Structured Output**: Uses GBNF grammar to ensure valid JSON responses.
- **Type-Safe State Machine**: Built on `framework` framework for compile-time safety.
- **DMs and group chats**: Works in 1:1 DMs and in server channels. Each conversation is keyed by its channel; in a group the model sees every message (prefixed with the sender's name) and decides for itself whether to reply, staying silent by replying with the marker `<empty>`.

## Quick Start

### Prerequisites

1.  **Docker** with buildx installed (Docker Desktop or Docker 19.03+).
2.  **Discord Token**: You need a Discord bot token.
3.  **Message Content Intent**: Enable the privileged **Message Content** intent for the bot in the Discord Developer Portal (Bot → Privileged Gateway Intents). Without it the bot can't read the text of group-channel messages that don't mention it.

### Installation & Build

1.  **Configure Token**:
    Copy `src/configuration.rs.template` to `src/configuration.rs` and add your token.
    ```bash
    cp src/configuration.rs.template src/configuration.rs
    # Edit src/configuration.rs with your DISCORD_TOKEN
    ```

    > [!IMPORTANT]
    > The token is currently compiled into the binary. You must rebuild the image if the token changes.

2.  **Build the Image**:
    The build process compiles the Rust binary and includes the ~8.4GB model file.

    Using **Just** (recommended):
    ```bash
    just build_push
    ```

    Or **Docker Compose**:
    ```bash
    docker-compose build
    ```

    Or **Docker Buildx**:
    ```bash
    docker buildx build --platform linux/amd64 -f Dockerfile -t bot-hive:latest ..
    ```

### Running the Bot

**Using Docker Compose**:
```bash
docker-compose up -d
```

**Using Docker Run**:
```bash
docker run -d \
  --name bot-hive \
  --restart unless-stopped \
  bot-hive:latest
```

## Configuration

### Environment Variables
- `PRIMARY_MODEL_PATH`: Path to the primary LLM model file inside the container (default: `/app/models/Qwen3.6-27B-Q4_K_M.gguf`).
- `RUST_LOG`: Log level (default: `info`).

### Session File Caching
To improve startup performance, the bot pre-evaluates the static base prompt (system instructions, JSON schema, tool definitions) and caches it to `resources/base_prompt.session` (~200MB).
- On startup, if the session file exists, it's loaded to skip re-evaluation.
- If missing or invalid, it falls back to full evaluation and regenerates the file.

### Web Search (SearxNG)

The `web_search` tool queries a [SearxNG](https://github.com/searxng/searxng) instance — a self-hosted metasearch engine — instead of a metered third-party API, so there's no external rate limit to trip.

The bot only needs **a reachable instance with JSON output enabled**:

- Point `SEARXNG_URL` (in `src/configuration.rs`) at the instance's base URL. The bot runs with host networking (`--network host`), so `localhost` is the host machine — the default `http://localhost:8080` reaches a SearxNG published on the host's port 8080. A remote instance works too (e.g. `https://search.example.com`).
- The instance **must** have JSON format enabled (it's off by default — see the `settings.yml` below).

> [!NOTE]
> **Running** SearxNG (Docker, bare metal, hosted) is out of scope here — see the [upstream docs](https://docs.searxng.org/). The steps below are just a convenience for a quick local instance.

A minimal local container:

1. **Create a config dir and `settings.yml`** that overrides only what we need on top of the image defaults:
   ```yaml
   # <config-dir>/settings.yml
   use_default_settings: true
   server:
     # Required: SearxNG won't start without one. Generate via `openssl rand -hex 32`.
     secret_key: "<random-hex>"
     # Listen on all interfaces inside the container so the published port works.
     bind_address: "0.0.0.0"
   search:
     # JSON is off by default; the bot's web_search calls the JSON endpoint, so enable it.
     formats:
       - html
       - json
   ```

2. **Run the container**, mounting that config and publishing port 8080 on the host:
   ```bash
   docker run -d --name searxng --restart unless-stopped \
     -p 8080:8080 \
     -v <config-dir>:/etc/searxng \
     searxng/searxng
   ```
   The bot runs with `--network host`, so the host's `:8080` is reachable at the default `SEARXNG_URL=http://localhost:8080`.

3. **Verify JSON output works:**
   ```bash
   curl "http://localhost:8080/search?q=test&format=json"   # → JSON with a "results" array
   ```
   A `403`/HTML response usually means JSON isn't enabled (check `search.formats`) or the limiter is blocking direct API calls (set `server.limiter: false` for a private instance).

## Architecture

### Directory Structure
```
chatbot/
├── src/
│   ├── main.rs                 # Entry point, Env initialization
- todo() -
```

### Core Components

1.  **State Machine (`framework`)**:
    Manages user states (`Idle`, `AwaitingLLMDecision`, `SendingMessage`, `RunningTool`) and ensures valid transitions. Each user has an independent state.

2.  **Services (`src/services/`)**:
    - **DiscordService**: Handles WebSocket events and HTTP requests via `serenity`.
    - **LlamaCppService**: Manages the local LLM context, session caching, and inference.

3.  **Actions (`src/actions/`)**:
    Bridges the state machine's "actions" to actual side effects (calling LLM, sending messages, executing tools).

### Flow Overview
1.  **Message Received**: Normalized and passed to `UserStateMachine`.
2.  **Decision**: State machine transitions to `AwaitingLLMDecision`, triggering `LlamaCppService`.
3.  **Inference**:
    - Loads cached session (base prompt).
    - Appends dynamic conversation history.
    - Generates response using `response.gbnf` grammar for structured JSON.
4.  **Action**:
    - **Message User Response**: Sends message to user.
    - **Tool Call**: Executes tool (e.g., weather), feeds result back to LLM, and repeats.

## Development

The project uses a workspace structure where `chatbot` is the main application member.

- **Build Base Image**: `just build_base`
- **Build & Push**: `just build_push`
