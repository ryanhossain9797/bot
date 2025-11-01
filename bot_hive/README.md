# Bot Hive - User Lifecycle Documentation

## Overview

Type-safe state machine that manages user interactions through distinct states and transitions using a generic async framework (`lib_hive`).

## User States

Defined in `src/models/user.rs:6-13`:

1. **Idle** - Default state, waiting for user interaction
2. **RespondingToMessage** - Processing user message and sending response
3. **WaitingToSayGoodbye** - 10-second delay before goodbye message
4. **SayingGoodbye** - Sending goodbye message before returning to Idle

## User Actions

Defined in `src/models/user.rs:15-21`:

- **NewMessage** - User sends message with `start_conversation` flag
- **Timeout** - Scheduled timeout event (for goodbye delay)
- **SendResult** - Result of sending a message (success/failure)

## State Flow

```
Idle → RespondingToMessage → WaitingToSayGoodbye → SayingGoodbye → Idle
```

### Transitions

1. **Idle → RespondingToMessage** (`src/life_cycles/user_life_cycle.rs:12-24`)
   - Trigger: NewMessage with `start_conversation: true`
   - Action: Spawn external operation to send bot response

2. **RespondingToMessage → WaitingToSayGoodbye** (`src/life_cycles/user_life_cycle.rs:25-32`)
   - Trigger: SendResult (response sent successfully)
   - Action: Schedule 10-second timeout

3. **WaitingToSayGoodbye → SayingGoodbye** (`src/life_cycles/user_life_cycle.rs:33-42`)
   - Trigger: Timeout
   - Action: Spawn external operation to send goodbye message

4. **SayingGoodbye → Idle** (`src/life_cycles/user_life_cycle.rs:43-47`)
   - Trigger: SendResult (goodbye sent)
   - Action: Return to Idle state

## Discord Communication

Uses **Serenity v0.12** for Discord integration via WebSocket (incoming) and HTTP (outgoing).

### Initialization

`src/external_connections/discord.rs:42-52` and `src/main.rs:40-48`

- Token from `configuration::client_tokens::DISCORD_TOKEN`
- Gateway Intent: `DIRECT_MESSAGES` only (DM-only bot)
- Event handler connects to user lifecycle
- Runs in spawned task via JoinSet

### Incoming Messages

`src/external_connections/discord.rs:13-26`

**Pipeline:**
1. Discord WebSocket → Handler::message()
2. Ignore bot messages
3. Normalize message (lowercase, trim, remove mentions/slashes, collapse spaces)
4. Create UserId(Discord, author_id_string)
5. Dispatch NewMessage to lifecycle with `start_conversation` flag (true if DM or bot mentioned)

### Outgoing Messages

`src/connectors/user_connector.rs:6-38`

**Pipeline:**
1. Parse UserId string → Discord UserId (u64)
2. Fetch user via HTTP API
3. Create/get DM channel
4. Send message via HTTP API
5. Return SendResult with success/error

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
    ↓ HTTP API
Discord Server (message delivered)
    ↓ SendResult
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

### Key Features

- **Platform-agnostic lifecycle** - Discord is one connector, Telegram planned
- **DM-only** - Only responds to direct messages
- **Concurrent users** - Each user has independent state
- **Deterministic** - Each state-action has exactly one outcome
- **Error handling** - Invalid transitions return errors

## File Structure

```
bot_hive/src/
├── main.rs                      - Entry point, initialization
├── configuration.rs             - Discord token config
├── external_connections/
│   └── discord.rs               - Client setup, event handler
├── connectors/
│   └── user_connector.rs        - Message sender
├── life_cycles/
│   └── user_life_cycle.rs       - State transitions, scheduling
└── models/
    └── user.rs                  - States, actions, types

lib_hive/src/
├── lib.rs                       - State machine framework
└── life_cycle_handle.rs         - Handle API
```

## Configuration

- **Goodbye delay:** 10 seconds (`src/life_cycles/user_life_cycle.rs:28`)
- Modify via `ChronoDuration::milliseconds(10_000)`

## Example Flow

1. User sends "Hello" on Discord
2. Handler normalizes to "hello", creates NewMessage action
3. Idle → RespondingToMessage, spawns response
4. Response sent: "You said hello"
5. RespondingToMessage → WaitingToSayGoodbye (10s timeout)
6. Timeout fires
7. WaitingToSayGoodbye → SayingGoodbye, spawns goodbye
8. Goodbye sent: "Goodbye"
9. SayingGoodbye → Idle
10. Ready for next conversation
