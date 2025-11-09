# Bot Hive - AI Chatbot with Local LLM

## Overview

Bot Hive is a Discord chatbot powered by a local Large Language Model (LLM) that provides conversational AI capabilities with conversation context management. It uses a type-safe state machine framework (`lib_hive`) to manage user interactions through distinct states and transitions.

The bot uses **Qwen2.5-14B-Instruct** (quantized Q4_K_M) running locally via `llama-cpp-2`, enabling private, offline AI conversations without relying on external APIs.

## Key Features

- **Local LLM Integration** - Uses Qwen2.5-14B-Instruct running locally via llama.cpp
- **Conversation Context** - Maintains compact conversation summaries for multi-turn conversations
- **Structured Output** - Uses GBNF grammar to ensure valid JSON responses with intent detection
- **Device Control Intents** - Can detect device control commands (extensible for smart home integration)
- **Type-Safe State Machine** - Built on `lib_hive` framework for compile-time safety
- **DM-Only Bot** - Responds only to direct messages or when mentioned
- **Automatic Timeouts** - Sends goodbye messages after 5 minutes of inactivity
- **Multi-User Support** - Each user has independent conversation state

## User States

Defined in `src/models/user.rs:28-31`:

1. **Idle** - Default state, waiting for user interaction
   - May contain optional conversation summary and timestamp
   - Automatically schedules timeout after 5 minutes of inactivity
   
2. **SendingMessage** - Processing user message and sending LLM response
   - Tracks whether this is a timeout-triggered goodbye message

## User Actions

Defined in `src/models/user.rs:44-51`:

- **NewMessage** - User sends a message with `start_conversation` flag
- **Timeout** - Scheduled timeout event (5 minutes after last activity)
- **SendResult** - Result of sending a message (contains updated conversation summary or error)

## State Flow

```
Idle → SendingMessage → Idle
         ↑                ↓
         └─── Timeout ────┘
```

### Transitions

1. **Idle → SendingMessage** (`src/life_cycles/user_life_cycle.rs:26-54`)
   - Trigger: `NewMessage` with `start_conversation: true`
   - Action: 
     - Retrieves previous conversation summary (if exists)
     - Spawns external operation to generate and send LLM response
     - Transitions to `SendingMessage(false)`

2. **SendingMessage → Idle** (`src/life_cycles/user_life_cycle.rs:56-72`)
   - Trigger: `SendResult` (response sent)
   - Action:
     - If successful: Stores updated conversation summary and timestamp, returns to `Idle`
     - If error: Returns to `Idle` without saving summary
     - If timeout-triggered: Clears conversation summary

3. **Idle → SendingMessage (Timeout)** (`src/life_cycles/user_life_cycle.rs:73-91`)
   - Trigger: `Timeout` (5 minutes after last activity)
   - Action:
     - Sends goodbye message mentioning relevant conversation context
     - Transitions to `SendingMessage(true)`

## LLM Integration

### Model Configuration

- **Model**: Qwen2.5-14B-Instruct-Q4_K_M.gguf (default)
- **Model Path**: Configurable via `MODEL_PATH` environment variable
- **Context Size**: 2048 tokens
- **Threading**: Uses all available CPU cores
- **Location**: `src/external_connections/llm.rs`

### Response Format

The LLM generates structured JSON responses with:

```json
{
  "updated_summary": "Compact conversation summary (machine-readable)",
  "response": "Human-readable response text",
  "intent": {
    "BasicConversation": {} | null,
    "ControlDevice": {"device": "...", "property": "...", "value": "..."} | null
  }
}
```

- **updated_summary**: Extremely compact, machine-readable format for context (e.g., `"usr:greet|dev:AC>temp=27"`)
- **response**: Natural language response to send to the user
- **intent**: One of two intents (exactly one must be non-null):
  - `BasicConversation`: General conversation, questions, greetings
  - `ControlDevice`: Device control commands (extensible for smart home)

### Grammar Constraints

Uses GBNF grammar (`grammars/response.gbnf`) to ensure valid JSON output and enforce the oneof intent pattern. This guarantees:
- Valid JSON structure
- Exactly one intent is non-null
- Proper device control structure when applicable

### Conversation Context

- **First message**: No summary provided, LLM starts fresh
- **Subsequent messages**: Previous compact summary included in prompt
- **Summary format**: Machine-readable shorthand (e.g., `"usr:greet|dev:AC=27|lights:on"`)
- **Timeout handling**: Goodbye messages reference conversation context

## Discord Communication

Uses **Serenity v0.12** for Discord integration via WebSocket (incoming) and HTTP (outgoing).

### Initialization

`src/external_connections/discord.rs:10-23` and `src/main.rs:40-54`

- Token from `configuration::client_tokens::DISCORD_TOKEN`
- Gateway Intent: `DIRECT_MESSAGES` only (DM-only bot)
- Event handler connects to user lifecycle
- Runs in spawned task via JoinSet
- LLM initialized once at startup and shared across all users

### Incoming Messages

`src/external_connections/discord.rs:39-51` and `filter()` function (lines 59-84)

**Pipeline:**
1. Discord WebSocket → `Handler::message()`
2. Ignore bot messages
3. Normalize message:
   - Convert to lowercase
   - Trim whitespace
   - Remove leading `/` (slash commands)
   - Remove bot mentions
   - Collapse multiple spaces to single space
4. Create `UserId(Discord, author_id_string)`
5. Determine `start_conversation`: `true` if DM or bot mentioned
6. Dispatch `NewMessage` to lifecycle

### Outgoing Messages

`src/connectors/user_connector.rs:189-240`

**Pipeline:**
1. Parse `UserId` string → Discord `UserId` (u64)
2. Fetch user via HTTP API
3. Create/get DM channel
4. Generate LLM response with conversation context
5. Send message via HTTP API
6. Return `SendResult` with updated summary or error

### Communication Flow

```
Discord Server
    ↓ WebSocket
Serenity Client
    ↓ Message Event
Handler::message()
    ↓ filter & normalize
user_life_cycle.act()
    ↓
State Machine
    ↓ spawn external operation
handle_bot_message()
    ↓ LLM Processing
get_response_from_llm()
    ↓ Grammar-constrained generation
Structured JSON Response
    ↓ HTTP API
Discord Server (message delivered)
    ↓ SendResult (with updated_summary)
State Machine (next state)
```

## Architecture

### State Machine Framework

`lib_hive/src/lib.rs`

- **Type-safe transitions** - Invalid state-action combos caught at compile time
- **Async-first** - All operations use tokio async/await
- **Channel-based** - mpsc channels for thread-safe state management
- **External operations** - Side effects separated from state logic
- **Per-user isolation** - Each user has independent state
- **Scheduled events** - Automatic timeout management

### Key Design Decisions

- **Platform-agnostic lifecycle** - Discord is one connector, Telegram planned (`UserChannel` enum)
- **DM-only** - Only responds to direct messages or when mentioned
- **Concurrent users** - Each user has independent state and conversation context
- **Deterministic** - Each state-action has exactly one outcome
- **Error handling** - Invalid transitions return errors, failed sends don't crash the bot
- **Shared LLM** - Single model instance shared across all users (initialized at startup)

## File Structure

```
bot_hive/
├── src/
│   ├── main.rs                      - Entry point, initialization, LLM setup
│   ├── configuration.rs             - Discord token config (copy from .template)
│   ├── external_connections/
│   │   ├── discord.rs               - Client setup, event handler, message filtering
│   │   └── llm.rs                   - LLM model loading and initialization
│   ├── connectors/
│   │   └── user_connector.rs        - Message sender, LLM response generation
│   ├── life_cycles/
│   │   └── user_life_cycle.rs       - State transitions, scheduling, timeout logic
│   └── models/
│       ├── user.rs                  - User states, actions, UserId, conversation summary
│       └── bot.rs                   - Bot handle (currently minimal)
├── grammars/
│   └── response.gbnf                - GBNF grammar for structured JSON output
├── models/
│   ├── Qwen2.5-14B-Instruct-Q4_K_M.gguf  - Default LLM model
│   └── README.md                    - Model information
└── Cargo.toml                       - Dependencies (Serenity, llama-cpp-2, tokio, etc.)

lib_hive/
└── src/
    ├── lib.rs                       - State machine framework
    └── life_cycle_handle.rs         - Handle API
```

## Configuration

### Environment Variables

- **MODEL_PATH**: Path to LLM model file (default: `models/Qwen2.5-14B-Instruct-Q4_K_M.gguf`)

### Configuration File

Copy `src/configuration.rs.template` to `src/configuration.rs` and set:
- `DISCORD_TOKEN`: Your Discord bot token

### Timeout Configuration

- **Goodbye delay**: 5 minutes (`src/life_cycles/user_life_cycle.rs:101`)
- Modify via `ChronoDuration::milliseconds(300_000)`

## Example Flow

1. User sends "Hello" via Discord DM
2. Handler normalizes to "hello", creates `NewMessage` action with `start_conversation: true`
3. `Idle(None)` → `SendingMessage(false)`, spawns LLM response generation
4. LLM generates: `{"updated_summary": "usr:greet", "response": "Hello! How can I help you?", ...}`
5. Response sent: "Hello! How can I help you?"
6. `SendingMessage(false)` → `Idle(Some(summary, timestamp))`
7. User sends "Set AC to 27 degrees"
8. LLM receives previous summary, generates: `{"updated_summary": "usr:greet|dev:AC>temp=27", "response": "Setting AC to 27 degrees", "intent": {"ControlDevice": {...}}}`
9. Response sent, summary updated
10. 5 minutes pass with no activity
11. `Timeout` fires
12. `Idle(Some(...))` → `SendingMessage(true)`, spawns goodbye message
13. LLM generates goodbye referencing conversation context
14. Goodbye sent, `SendingMessage(true)` → `Idle(None)` (summary cleared)
15. Ready for next conversation

## Dependencies

- **tokio**: Async runtime
- **serenity**: Discord API client
- **llama-cpp-2**: Local LLM inference
- **lib_hive**: Type-safe state machine framework
- **serde/serde_json**: JSON serialization
- **chrono**: Time handling for timeouts
- **regex**: Message normalization
- **anyhow**: Error handling

## Future Enhancements

- Telegram connector implementation
- Actual device control integration (currently only detects intents)
- Multiple model support/configurable models
- Conversation history persistence
- Admin commands and configuration
