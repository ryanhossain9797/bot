# Bot Hive - AI Chatbot with Local LLM

## Overview

Bot Hive is a Discord chatbot powered by a local Large Language Model (LLM) that provides conversational AI capabilities with tool calling support. It uses a type-safe state machine framework (`framework`) to manage user interactions through distinct states and transitions.

The bot uses **Qwen3.6-35B-A3B** (quantized Q8_0) running locally via `llama-cpp-2`, enabling private, offline AI conversations without relying on external APIs.

## Features

- **Local LLM Integration**: Uses Qwen3.6-35B-A3B running locally via llama.cpp.
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
- `PRIMARY_MODEL_PATH`: Path to the primary LLM model file inside the container (default: `/app/models/Qwen3.6-35B-A3B-Q8_0.gguf`).
- `RUST_LOG`: Log level (default: `info`).

### Session File Caching
To improve startup performance, the bot pre-evaluates the static base prompt (system instructions, JSON schema, tool definitions) and caches it to `resources/base_prompt.session` (~200MB).
- On startup, if the session file exists, it's loaded to skip re-evaluation.
- If missing or invalid, it falls back to full evaluation and regenerates the file.

### Web Search (SearxNG)

The `web_search` tool queries a [SearxNG](https://github.com/searxng/searxng) instance — a self-hosted metasearch engine — instead of a metered third-party API, so there's no external rate limit to trip.

**The bot manages SearxNG itself** — the same way it manages the bash sandbox. When `SEARXNG_URL` points at localhost (the default `http://localhost:8080`), the bot builds a `bot-searxng` image (`searxng/searxng` + a baked `settings.yml` with JSON enabled and a build-time-random `secret_key`) and runs it on the host Docker daemon via the mounted socket. Before every `web_search`, and at startup, `ensure_searxng()` recovers it if it's missing or stopped, and the container runs with `--restart unless-stopped`. So an accidental `docker rm`/stop of `bot-searxng`, or a daemon restart, self-heals without touching the bot.

- The staged build context lives at `searxng/` (`Dockerfile` + `settings.yml`), copied into the bot image at `/app/searxng`.
- **Remote instances are left unmanaged:** set `SEARXNG_URL` to a non-localhost URL (e.g. `https://search.example.com`) and the bot skips container management and just queries it. That instance must have JSON format enabled itself.

> [!NOTE]
> Verify the JSON endpoint (locally: `curl "http://localhost:8080/search?q=test&format=json"` → JSON with a `results` array). A `403`/HTML response usually means JSON isn't enabled (`search.formats`) or the limiter is blocking direct API calls (`server.limiter: false`) — both are already set in the baked `settings.yml`.

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
