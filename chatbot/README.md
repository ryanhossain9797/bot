# Bot Hive - AI Chatbot with Local LLM

## Overview

Bot Hive is a Discord chatbot powered by a local Large Language Model (LLM) that provides conversational AI capabilities with tool calling support. It uses a type-safe state machine framework (`framework`) to manage user interactions through distinct states and transitions.

The bot uses **Qwen2.5-14B-Instruct** (quantized Q4_K_M) running locally via `llama-cpp-2`, enabling private, offline AI conversations without relying on external APIs.

## Features

- **Local LLM Integration**: Uses Qwen2.5-14B-Instruct running locally via llama.cpp.
- **Tool Calling**: Supports multi-turn tool execution (e.g., weather lookup).
- **Conversation Context**: Maintains conversation summaries for multi-turn conversations.
- **Structured Output**: Uses GBNF grammar to ensure valid JSON responses.
- **Type-Safe State Machine**: Built on `framework` framework for compile-time safety.
- **DM-Only Bot**: Responds only to direct messages or when mentioned.

## Quick Start

### Prerequisites

1.  **Docker** with buildx installed (Docker Desktop or Docker 19.03+).
2.  **Discord Token**: You need a Discord bot token.

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
- `MODEL_PATH`: Path to the LLM model file inside the container (default: `/app/models/Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf`).
- `RUST_LOG`: Log level (default: `info`).

### Session File Caching
To improve startup performance, the bot pre-evaluates the static base prompt (system instructions, JSON schema, tool definitions) and caches it to `resources/base_prompt.session` (~200MB).
- On startup, if the session file exists, it's loaded to skip re-evaluation.
- If missing or invalid, it falls back to full evaluation and regenerates the file.

## Architecture

### Directory Structure
```
chatbot/
├── src/
│   ├── main.rs                 # Entry point, Env initialization
│   ├── configuration.rs        # Token configuration
│   ├── services/               # Core services
│   │   ├── discord.rs          # Discord client & event handler
│   │   └── llama_cpp.rs        # Local LLM inference service
│   │
│   ├── externals/              # External operation handlers
│   │   ├── llm_external.rs     # LLM decision making
│   │   ├── message_external.rs # Message sending
│   │   └── tool_call_external.rs  # Tool execution
│   │
│   ├── state_machines/         # User state machine logic
│   └── models/                 # Data models
├── grammars/
│   └── response.gbnf           # GBNF grammar for JSON output
└── resources/
    └── base_prompt.session     # Pre-cached prompt session
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
    - **Final Response**: Sends message to user.
    - **Tool Call**: Executes tool (e.g., weather), feeds result back to LLM, and repeats.

## Development

The project uses a workspace structure where `chatbot` is the main application member.

- **Build Base Image**: `just build_base`
- **Build & Push**: `just build_push`
