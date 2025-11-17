# Bot Hive - AI Chatbot with Local LLM

## Overview

Bot Hive is a Discord chatbot powered by a local Large Language Model (LLM) that provides conversational AI capabilities with tool calling support. It uses a type-safe state machine framework (`lib_hive`) to manage user interactions through distinct states and transitions.

The bot uses **Qwen2.5-14B-Instruct** (quantized Q4_K_M) running locally via `llama-cpp-2`, enabling private, offline AI conversations without relying on external APIs.

## Key Features

- **Local LLM Integration** - Uses Qwen2.5-14B-Instruct running locally via llama.cpp
- **Tool Calling** - Supports multi-turn tool execution with weather lookup functionality
- **Conversation Context** - Maintains conversation summaries for multi-turn conversations
- **Structured Output** - Uses GBNF grammar to ensure valid JSON responses
- **Type-Safe State Machine** - Built on `lib_hive` framework for compile-time safety
- **DM-Only Bot** - Responds only to direct messages or when mentioned
- **Automatic Timeouts** - Sends goodbye messages after 5 minutes of inactivity
- **Multi-User Support** - Each user has independent conversation state
- **Error Recovery** - Force reset mechanism prevents stuck states

## User States

Defined in `src/models/user.rs:28-46`:

1. **Idle** - Default state, waiting for user interaction
   - May contain optional conversation summary and timestamp
   - Automatically schedules timeout after 5 minutes of inactivity
   
2. **AwaitingLLMDecision** - Waiting for LLM to decide on response or tool call
   - Tracks whether this is a timeout-triggered request
   - Maintains history of previous tool calls and results

3. **SendingMessage** - Sending a message to the user
   - Stores the outcome (Final or IntermediateToolCall) to determine next state
   - Tracks conversation context and tool call history

4. **RunningTool** - Executing a tool call
   - Maintains conversation context and tool call history
   - Transitions back to AwaitingLLMDecision after tool completion

## User Actions

Defined in `src/models/user.rs:82-93`:

- **ForceReset** - Force reset to Idle state (used for stuck state recovery)
- **NewMessage** - User sends a message with `start_conversation` flag
- **Timeout** - Scheduled timeout event (5 minutes after last activity)
- **LLMDecisionResult** - Result from LLM connector (contains updated summary and outcome)
- **MessageSent** - Result from message connector (success or failure, errors are ignored)
- **ToolResult** - Result from tool execution (contains tool output)

## State Flow

```
Idle → AwaitingLLMDecision → SendingMessage → Idle
         ↑                        ↓
         └─── RunningTool ────────┘
         ↑                ↓
         └─── ToolResult ─┘
```

### Transitions

1. **Idle → AwaitingLLMDecision** (`src/life_cycles/user_life_cycle.rs:36-67`)
   - Trigger: `NewMessage` with `start_conversation: true` or `Timeout`
   - Action: 
     - Retrieves previous conversation summary (if exists)
     - Spawns external operation to get LLM decision
     - Transitions to `AwaitingLLMDecision`

2. **AwaitingLLMDecision → SendingMessage or RunningTool** (`src/life_cycles/user_life_cycle.rs:69-168`)
   - Trigger: `LLMDecisionResult`
   - Action:
     - If outcome is `Final` with message → go to `SendingMessage`
     - If outcome is `IntermediateToolCall` with message → go to `SendingMessage`
     - If outcome is `IntermediateToolCall` without message (silent) → go directly to `RunningTool`

3. **SendingMessage → Idle or RunningTool** (`src/life_cycles/user_life_cycle.rs:169-197`)
   - Trigger: `MessageSent` (errors are ignored)
   - Action:
     - If outcome is `Final` → transition to `Idle` (preserving conversation if not timeout)
     - If outcome is `IntermediateToolCall` → transition to `RunningTool` and execute tool

4. **RunningTool → AwaitingLLMDecision** (`src/life_cycles/user_life_cycle.rs:198-240`)
   - Trigger: `ToolResult`
   - Action:
     - Adds tool result to previous tool calls history
     - Spawns external operation to get next LLM decision with tool results
     - Transitions back to `AwaitingLLMDecision` (tool call loop continues)

5. **Force Reset** (`src/life_cycles/user_life_cycle.rs:26-32`)
   - Trigger: `ForceReset` (scheduled after 120s in AwaitingLLMDecision, SendingMessage, or RunningTool)
   - Action: Resets to `Idle(None)` to prevent stuck states

## LLM Integration

The Bot Hive integrates a Large Language Model (LLM) directly into its core functionality for conversational AI and tool orchestration. This is achieved through local inference, ensuring privacy and reducing reliance on external API services.

### Model Loading and Configuration

- **Technology**: Utilizes the `llama_cpp_2` Rust crate, a high-performance binding for `llama.cpp`, to run quantized GGUF models locally.
- **Model**: Defaults to `Qwen2.5-14B-Instruct-Q4_K_M.gguf`.
- **Model Path**: Configurable via the `MODEL_PATH` environment variable (default: `models/Qwen2.5-14B-Instruct-Q4_K_M.gguf`).
- **Context Size**: Configured for 2048 tokens.
- **Threading**: Leverages all available CPU cores for efficient processing.
- **Location**: The model loading logic resides in `src/external_connections/llm.rs`.

### LLM Workflow

1.  **Prompt Construction**: For each user interaction, a detailed system prompt is dynamically constructed in `src/connectors/llm_connector.rs`. This prompt includes:
    *   The bot's persona and instructions.
    *   A clear definition of the expected JSON output format.
    *   Details about available tools (e.g., `GetWeather`) and their usage rules.
    *   The current conversation summary (`updated_summary`) for context.
    *   A history of previous tool calls and their results (`previous_tool_calls`).

2.  **Inference and Structured Output**:
    *   The constructed prompt is fed to the locally loaded LLM.
    *   Crucially, a GBNF (GGML BNF) grammar, defined in `grammars/response.gbnf`, is applied during the LLM's token generation process. This grammar strictly enforces the required JSON schema, guaranteeing that the LLM's output is always valid and adheres to the expected `LLMResponse` structure.

3.  **Decision Making**: The LLM's structured JSON output contains two key elements:
    *   `updated_summary`: A concise summary of the conversation, maintained across turns to provide ongoing context.
    *   `outcome`: This dictates the bot's next action, which can be either:
        *   `Final`: A complete, direct response to the user.
        *   `IntermediateToolCall`: An instruction to execute a specific tool (e.g., `GetWeather`) before formulating a final response.

### Response Format

The LLM generates structured JSON responses with:

```json
{
  "updated_summary": "Brief conversation summary",
  "outcome": {
    "Final": {
      "response": "Your response to the user"
    }
  }
}
```

Or for tool calls:

```json
{
  "updated_summary": "Brief conversation summary",
  "outcome": {
    "IntermediateToolCall": {
      "maybe_intermediate_response": "Checking weather for London" | null,
      "tool_call": {
        "GetWeather": {
          "location": "London"
        }
      }
    }
  }
}
```

- **updated_summary**: Brief summary of conversation context for future reference
- **outcome**: Either `Final` (complete response) or `IntermediateToolCall` (requires tool execution)
  - `Final`: Contains the response string to send to user
  - `IntermediateToolCall`: Contains optional intermediate message and tool call to execute

### Grammar Constraints

Uses GBNF grammar (`grammars/response.gbnf`) to ensure valid JSON output. This guarantees:
- Valid JSON structure
- Proper outcome format (Final or IntermediateToolCall)
- Valid tool call structure

### Tool Calling

The bot supports a multi-turn tool calling flow:

1. User asks for weather → LLM decides to call `GetWeather`
2. Bot sends optional intermediate message (e.g., "Checking weather for London")
3. Tool executes → fetches weather from wttr.in API
4. Tool result added to conversation history
5. LLM receives tool result and generates final response
6. Bot sends final response to user

The LLM can chain multiple tool calls if needed, accumulating results until it can provide a final response.

### Available Tools

- **GetWeather**: Fetches weather information for a specific location
  - Requires a specific geographic location (city name, place name)
  - Uses wttr.in API (free, no API key required)
  - Returns formatted weather data (condition, temperature, wind, humidity)

### Conversation Context

- **First message**: No summary provided, LLM starts fresh
- **Subsequent messages**: Previous summary included in prompt
- **Tool call history**: Previous tool calls and results are included in context
- **Summary format**: Brief, informative summary maintained across conversation
- **Timeout handling**: Goodbye messages reference conversation context

## Discord Communication

Uses **Serenity v0.12** for Discord integration via WebSocket (incoming) and HTTP (outgoing).

### Initialization

`src/external_connections/discord.rs` and `src/main.rs`

- Token from `configuration::client_tokens::DISCORD_TOKEN`
- Gateway Intent: `DIRECT_MESSAGES` only (DM-only bot)
- Event handler connects to user lifecycle
- Runs in spawned task via JoinSet
- LLM initialized once at startup and shared across all users

### Incoming Messages

`src/external_connections/discord.rs`

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

`src/connectors/message_connector.rs`

**Pipeline:**
1. Parse `UserId` string → Discord `UserId` (u64)
2. Fetch user via HTTP API
3. Create/get DM channel
4. Send message via HTTP API
5. Return `MessageSent` action (errors are ignored)

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
get_llm_decision()
    ↓ LLM Processing
get_response_from_llm()
    ↓ Grammar-constrained generation
Structured JSON Response
    ↓ LLMDecisionResult
State Machine
    ↓ if message exists
send_message()
    ↓ HTTP API
Discord Server (message delivered)
    ↓ MessageSent
State Machine (next state)
    ↓ if tool call
execute_tool()
    ↓ Tool execution
ToolResult
    ↓ back to LLM
State Machine (loop until Final)
```

## Architecture

### Connectors

The bot uses a connector-based architecture for external operations:

- **llm_connector** (`src/connectors/llm_connector.rs`): Handles LLM decision-making
  - Takes message, summary, and tool call history
  - Returns `LLMDecisionResult` with updated summary and outcome

- **message_connector** (`src/connectors/message_connector.rs`): Handles sending messages
  - Takes user ID and message content
  - Returns `MessageSent` action (errors are ignored)

- **tool_call_connector** (`src/connectors/tool_call_connector.rs`): Handles tool execution
  - Executes tool calls (currently only GetWeather)
  - Returns `ToolResult` with tool output

### State Machine Framework

`lib_hive/src/lib.rs`

- **Type-safe transitions** - Invalid state-action combos caught at compile time
- **Async-first** - All operations use tokio async/await
- **Channel-based** - mpsc channels for thread-safe state management
- **External operations** - Side effects separated from state logic
- **Per-user isolation** - Each user has independent state
- **Scheduled events** - Automatic timeout and force reset management

### Key Design Decisions

- **Platform-agnostic lifecycle** - Discord is one connector, Telegram planned (`UserChannel` enum)
- **DM-only** - Only responds to direct messages or when mentioned
- **Concurrent users** - Each user has independent state and conversation context
- **Deterministic** - Each state-action has exactly one outcome
- **Error handling** - Invalid transitions return errors, failed sends don't crash the bot
- **Shared LLM** - Single model instance shared across all users (initialized at startup)
- **Tool call loop** - LLM can chain multiple tool calls until final response
- **Force reset** - Prevents stuck states with 120-second timeout

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
│   │   ├── llm_connector.rs         - LLM decision-making
│   │   ├── message_connector.rs     - Message sending
│   │   └── tool_call_connector.rs  - Tool execution
│   ├── life_cycles/
│   │   └── user_life_cycle.rs       - State transitions, scheduling, timeout logic
│   └── models/
│       ├── user.rs                  - User states, actions, UserId, ToolCall, MessageOutcome
│       └── bot.rs                   - Bot handle (currently minimal)
├── grammars/
│   └── response.gbnf                - GBNF grammar for structured JSON output
├── models/
│   ├── Qwen2.5-14B-Instruct-Q4_K_M.gguf  - Default LLM model
│   └── README.md                    - Model information
└── Cargo.toml                       - Dependencies (Serenity, llama-cpp-2, tokio, reqwest, etc.)

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

- **Goodbye delay**: 5 minutes (`src/life_cycles/user_life_cycle.rs:273`)
- **Force reset delay**: 120 seconds (`src/life_cycles/user_life_cycle.rs:280`)
- Modify via `ChronoDuration::milliseconds(...)`

## Example Flow

1. User sends "What's the weather in London?" via Discord DM
2. Handler normalizes message, creates `NewMessage` action with `start_conversation: true`
3. `Idle(None)` → `AwaitingLLMDecision(false)`, spawns LLM decision
4. LLM generates: `{"updated_summary": "User asked about weather in London", "outcome": {"IntermediateToolCall": {"maybe_intermediate_response": "Checking weather for London", "tool_call": {"GetWeather": {"location": "London"}}}}}}`
5. `AwaitingLLMDecision` → `SendingMessage`, sends "Checking weather for London"
6. `SendingMessage` → `RunningTool`, executes GetWeather tool
7. Tool fetches weather: "Clear +15°C 10km/h 65%"
8. `RunningTool` → `AwaitingLLMDecision`, spawns LLM decision with tool result
9. LLM generates: `{"updated_summary": "User asked about weather in London, got weather data", "outcome": {"Final": {"response": "The weather in London is clear with a temperature of 15°C, wind at 10km/h, and 65% humidity."}}}`
10. `AwaitingLLMDecision` → `SendingMessage`, sends final response
11. `SendingMessage` → `Idle(Some(summary, timestamp))`
12. 5 minutes pass with no activity
13. `Timeout` fires
14. `Idle(Some(...))` → `AwaitingLLMDecision(true)`, spawns goodbye message
15. LLM generates goodbye referencing conversation context
16. Goodbye sent, `SendingMessage(true)` → `Idle(None)` (summary cleared)
17. Ready for next conversation

## Dependencies

- **tokio**: Async runtime
- **serenity**: Discord API client
- **llama-cpp-2**: Local LLM inference
- **lib_hive**: Type-safe state machine framework
- **serde/serde_json**: JSON serialization
- **chrono**: Time handling for timeouts
- **reqwest**: HTTP client for weather API
- **urlencoding**: URL encoding for API requests
- **anyhow**: Error handling

## Future Enhancements

- Telegram connector implementation
- Additional tool integrations
- Multiple model support/configurable models
- Conversation history persistence
- Admin commands and configuration
- Context pooling for faster LLM inference
