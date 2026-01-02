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
- **Model Path**: Configurable via the `MODEL_PATH` environment variable (default: `./models/Qwen2.5-14B-Instruct-Q4_K_M.gguf`).
- **Context Size**: Configured for 2048 tokens.
- **Threading**: Leverages all available CPU cores for efficient processing.
- **Location**: The model loading logic resides in `src/external_connections/llm.rs`.

### Base Prompt Session File Caching

The bot implements a significant performance optimization using session file caching:

- **Static Base Prompt**: The system prompt with bot persona, JSON schema, and tool definitions is defined in `BasePrompt::build_prompt()` (`src/external_connections/llm.rs:33-95`).
- **Session File Creation**: At startup, the base prompt is pre-evaluated once and the KV cache is saved to `resources/base_prompt.session` (~200MB binary file).
- **Runtime Loading**: On each inference, the session file is loaded instead of re-evaluating the static prompt, saving significant computation time.
- **Fallback Mechanism**: If session file loading fails, the system automatically falls back to full prompt evaluation with a warning logged.
- **Dynamic Prompt**: Only the conversation-specific parts (summary, tool call history, user message) are tokenized and evaluated for each request.

This optimization dramatically reduces inference latency since the static system prompt (which is large and doesn't change) is evaluated once at startup rather than for every user message.

### LLM Workflow

1.  **Initialization** (`src/main.rs:34-56`):
    *   At startup, the `BasePrompt` containing the static system prompt is created.
    *   The system prompt is pre-evaluated and saved as a session file (`resources/base_prompt.session`).
    *   If session file creation fails, the bot continues with a warning and falls back to full prompt evaluation.

2.  **Prompt Construction** (`src/connectors/llm_connector.rs:44-53`):
    *   **Static Base Prompt** (from session file): Contains bot persona, JSON schema, tool definitions, and instructions - evaluated once at startup.
    *   **Dynamic Prompt** (evaluated per request): Built using `build_dynamic_prompt()` and includes:
        *   The current conversation summary (`updated_summary`) for context.
        *   A history of previous tool calls and their results (`previous_tool_calls`).
        *   The user's current message.
        *   Proper ChatML formatting (`<|im_start|>user` / `<|im_end|>` / `<|im_start|>assistant`).

3.  **Session Loading and Inference** (`src/connectors/llm_connector.rs:84-111`):
    *   Attempt to load the session file with pre-evaluated base prompt KV cache.
    *   On success: Only tokenize and evaluate the dynamic prompt (fast path).
    *   On failure: Fall back to full prompt evaluation with warning (slower, but functional).
    *   Context continues from the loaded session position, dramatically reducing computation.

4.  **Structured Output with Grammar** (`src/connectors/llm_connector.rs:125-162`):
    *   A GBNF (GGML BNF) grammar from `grammars/response.gbnf` is applied during token generation.
    *   The grammar strictly enforces the required JSON schema, guaranteeing valid output.
    *   Temperature varies slightly (0.2-0.4) for natural responses while maintaining JSON validity.

5.  **Decision Making**: The LLM's structured JSON output contains two key elements:
    *   `updated_summary`: Full conversation history with recent messages kept verbatim (last 5-10 turns), older messages compressed.
    *   `outcome`: Dictates the bot's next action:
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

- **First message**: No summary provided, LLM starts fresh with "NO PREVIOUS CONVERSATION"
- **Subsequent messages**: Previous summary included in dynamic prompt
- **Tool call history**: Previous tool calls and results are included in dynamic prompt
- **Summary format**: Recent messages (last 5-10 turns) kept VERBATIM with exact wording, older messages compressed
  - Format: "Recent conversation:\n1. User: [exact message]\nAssistant: [exact response]\n2. ..."
  - This ensures the LLM has full context for recent exchanges
- **Timeout handling**: Goodbye messages reference conversation context from summary

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
- **Session file caching** - Static base prompt pre-evaluated at startup, only dynamic parts processed per request
- **Graceful degradation** - Session file loading failures automatically fall back to full prompt evaluation
- **Tool call loop** - LLM can chain multiple tool calls until final response
- **Force reset** - Prevents stuck states with 120-second timeout

## File Structure

```
bot_hive/
├── src/
│   ├── main.rs                      - Entry point, initialization, LLM setup, session file creation
│   ├── configuration.rs             - Discord token config (copy from .template)
│   ├── external_connections/
│   │   ├── discord.rs               - Client setup, event handler, message filtering
│   │   └── llm.rs                   - LLM model loading, BasePrompt, session file creation
│   ├── connectors/
│   │   ├── llm_connector.rs         - LLM decision-making, prompt building, session loading
│   │   ├── message_connector.rs     - Message sending
│   │   └── tool_call_connector.rs  - Tool execution
│   ├── life_cycles/
│   │   └── user_life_cycle.rs       - State transitions, scheduling, timeout logic
│   └── models/
│       ├── user.rs                  - User states, actions, UserId, ToolCall, MessageOutcome
│       └── bot.rs                   - Bot handle (currently minimal)
├── grammars/
│   └── response.gbnf                - GBNF grammar for structured JSON output
├── resources/
│   └── base_prompt.session          - Pre-evaluated KV cache for base prompt (~200MB, auto-generated)
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

- **MODEL_PATH**: Path to LLM model file (default: `./models/Qwen2.5-14B-Instruct-Q4_K_M.gguf`)

### Configuration File

Copy `src/configuration.rs.template` to `src/configuration.rs` and set:
- `DISCORD_TOKEN`: Your Discord bot token

### Session File

- **Location**: `resources/base_prompt.session`
- **Auto-generated**: Created automatically at bot startup
- **Size**: ~200MB (contains pre-evaluated KV cache for base prompt)
- **Regeneration**: Delete the file and restart the bot to regenerate
- **Not in Git**: This file should not be committed (binary, large, model-specific)

### Timeout Configuration

- **Goodbye delay**: 5 minutes (`src/life_cycles/user_life_cycle.rs:273`)
- **Force reset delay**: 120 seconds (`src/life_cycles/user_life_cycle.rs:280`)
- Modify via `ChronoDuration::milliseconds(...)`

## Performance Optimization: Session File Caching

The bot implements a sophisticated caching mechanism to dramatically improve inference performance:

### How It Works

1. **Startup Phase** (`src/main.rs:39-48`):
   - Base prompt is constructed with system instructions, JSON schema, tool definitions
   - Context is created and the base prompt is tokenized (~700 tokens)
   - Tokens are decoded and the resulting KV cache is saved to `resources/base_prompt.session`
   - Session file size: ~200MB (contains pre-computed attention states)

2. **Runtime Phase** (`src/connectors/llm_connector.rs:84-111`):
   - For each user request, load the session file (instant, no computation)
   - Only tokenize and evaluate the dynamic prompt (~100-300 tokens depending on conversation)
   - Continue generation from where the session left off

### Performance Impact

- **Without caching**: Every request evaluates ~700 static tokens + dynamic tokens
- **With caching**: Every request evaluates only ~100-300 dynamic tokens
- **Speedup**: 2-3x faster for typical requests
- **Memory**: Session file is ~200MB on disk, loaded into KV cache during inference

### Fallback Behavior

If session file loading fails (corrupted file, wrong model, etc.):
- Warning is logged to stderr
- System automatically falls back to full prompt evaluation
- Bot continues functioning normally, just slower
- Session file can be regenerated by restarting the bot

## Example Flow

1. **Bot Startup**: Session file created with pre-evaluated base prompt
2. User sends "What's the weather in London?" via Discord DM
3. Handler normalizes message, creates `NewMessage` action with `start_conversation: true`
4. `Idle(None)` → `AwaitingLLMDecision(false)`, spawns LLM decision
5. LLM loads session file (base prompt), evaluates only dynamic prompt (user message)
6. LLM generates: `{"updated_summary": "Recent conversation:\n1. User: What's the weather in London?\nAssistant: Checking weather for London", "outcome": {"IntermediateToolCall": {"maybe_intermediate_response": "Checking weather for London", "tool_call": {"GetWeather": {"location": "London"}}}}}}`
7. `AwaitingLLMDecision` → `SendingMessage`, sends "Checking weather for London"
8. `SendingMessage` → `RunningTool`, executes GetWeather tool
9. Tool fetches weather: "Clear +15°C 10km/h 65%"
10. `RunningTool` → `AwaitingLLMDecision`, spawns LLM decision with tool result
11. LLM loads session file again, evaluates dynamic prompt with tool results
12. LLM generates: `{"updated_summary": "Recent conversation:\n1. User: What's the weather in London?\nAssistant: Checking weather for London\n2. User: (tool result)\nAssistant: The weather in London is clear with a temperature of 15°C, wind at 10km/h, and 65% humidity.", "outcome": {"Final": {"response": "The weather in London is clear with a temperature of 15°C, wind at 10km/h, and 65% humidity."}}}`
13. `AwaitingLLMDecision` → `SendingMessage`, sends final response
14. `SendingMessage` → `Idle(Some(summary, timestamp))`
15. 5 minutes pass with no activity
16. `Timeout` fires
17. `Idle(Some(...))` → `AwaitingLLMDecision(true)`, spawns goodbye message
18. LLM generates goodbye referencing conversation context
19. Goodbye sent, `SendingMessage(true)` → `Idle(None)` (summary cleared)
20. Ready for next conversation

## Dependencies

- **tokio**: Async runtime
- **serenity**: Discord API client
- **llama-cpp-2**: Local LLM inference with session file support
- **lib_hive**: Type-safe state machine framework
- **serde/serde_json**: JSON serialization
- **chrono**: Time handling for timeouts
- **reqwest**: HTTP client for weather API
- **urlencoding**: URL encoding for API requests
- **anyhow**: Error handling
- **num_cpus**: CPU core detection for optimal threading
- **rand**: Random number generation for temperature variation
- **once_cell**: Global singleton initialization (Env struct)

## Future Enhancements

- Telegram connector implementation
- Additional tool integrations (web search, calculations, code execution)
- Multiple model support/configurable models
- Conversation history persistence (database storage)
- Admin commands and configuration (runtime config changes)
- Parallel session contexts (multiple pre-cached prompts for different bot personas)
- Session file versioning and validation
