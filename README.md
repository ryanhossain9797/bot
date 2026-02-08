# Bot Hive - AI Chatbot with Local LLM

A Rust-based Discord chatbot powered by a local Large Language Model, featuring an agent-based architecture and type-safe state machine framework.

## Overview

This is a multi-component project that provides a sophisticated AI chatbot experience:

- **Discord Integration**: Responds to direct messages and mentions
- **Local LLM**: Uses Qwen2.5-14B-Instruct running locally via llama.cpp
- **Tool Calling**: Supports multi-turn tool execution (weather, web search, etc.)
- **Memory Systems**: Short-term and long-term memory via LanceDB with embeddings
- **Type-Safe Architecture**: Built on a custom Rust state machine framework

## Project Structure

```
bot/
├── chatbot/                 # Main Discord chatbot application
│   ├── src/
│   │   ├── agents/         # Thinking and executor agents
│   │   ├── externals/       # External operations (LLM, tools, memory)
│   │   ├── models/         # Data models
│   │   ├── services/       # External service integrations
│   │   ├── state_machines/ # User and bot state machines
│   │   ├── agents.rs       # Agent traits and implementation
│   │   ├── main.rs         # Entry point
│   │   └── configuration.rs
│   ├── grammars/           # GBNF grammars for structured output
│   ├── models/             # GGUF model files
│   ├── resources/          # Session caches and resources
│   ├── Dockerfile          # Multi-stage build
│   ├── Dockerfile.base     # Base image with Rust + llama.cpp
│   ├── Cargo.toml          # Workspace member
│   └── Justfile            # Build automation
│
└── framework/              # Reusable state machine library
```

## Components

### Chatbot (`chatbot/`)

The main application that orchestrates:

- **DiscordService**: Handles WebSocket events and HTTP via `serenity`
- **LlamaCppService**: Manages local LLM inference with session caching
- **LanceService**: Vector database for memory/embedding storage
- **Thinking Agent**: Decision-making based on conversation context
- **Executor Agent**: Tool execution and response formatting

#### Available Tools

| Tool | Description |
|------|-------------|
| `message-user` | Send a response to the user |
| `get-weather` | Look up weather for a city |
| `web-search` | Search the web for information |
| `visit-url` | Fetch and extract content from a URL |
| `recall-short-term` | Retrieve recent conversation context |
| `recall-long-term` | Search long-term memory by topic |

### Framework (`framework/`)

A reusable Rust library providing:

- **Type-safe State Machines**: Compile-time safety for state transitions
- **Scheduled Operations**: Time-based wakeups
- **Entity Handles**: Actor-style message passing

## Architecture

### Agent Flow

```
User Message → DiscordService → UserStateMachine → ThinkingAgent
                                                    ↓
                              ┌─────────────────────┼─────────────────────┐
                              ↓                     ↓                     ↓
                      message-user            Tool Call            Recall Memory
                              ↓                     ↓                     ↓
                      DiscordService         ExecutorAgent         LanceService
                              ↓                     ↓                     ↓
                              └─────────────────────┼─────────────────────┘
                                                    ↓
                                              Final Response
                                                    ↓
                                              DiscordService
```

### State Machine Framework

The framework provides:
- `StateMachineHandle`: Send actions to a state machine by ID
- `Transition`: Define state transitions with side effects
- `Schedule`: Time-based wakeups for delayed actions
- `Activity`: Actions, scheduled wakeups, or deletion

## Building

### Prerequisites

- Docker with buildx
- Discord bot token

### Quick Build

```bash 
cd chatbot
just build_base
just deploy_local
```

### Configuration

1. Copy `chatbot/src/configuration.rs.template` to `chatbot/src/configuration.rs`
2. Add your Discord token
3. Rebuild the image

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `MODEL_PATH` | Path to GGUF model | `models/Llama-3.3-70B-Instruct-Q4_K_M.gguf` |
| `RUST_LOG` | Log level | `info` |

## Dependencies

### Runtime

- **llama-cpp-2**: Local LLM inference (Vulkan support)
- **serenity**: Discord API client
- **lancedb**: Vector database for memory
- **fastembed**: Embedding generation
- **tokio**: Async runtime

## License

MIT
