# Session File Setup Proposal for Bot Hive

## Overview

This document proposes how to integrate session file caching into Bot Hive to significantly improve inference performance by avoiding re-evaluation of the large, static system prompt on every inference call.

## Current State Analysis

### Current LLM Inference Flow

1. **Every inference call** (`get_response_from_llm()` in `src/connectors/llm_connector.rs`):
   - Creates a fresh context
   - Builds the full prompt (system prompt + summary + tool calls + user message)
   - Tokenizes the entire prompt
   - Decodes all tokens (including the large static system prompt)
   - Generates response

2. **System Prompt Structure**:
   - **Static part** (~2000+ tokens): Bot persona, instructions, JSON format, examples, rules
   - **Dynamic parts**:
     - Conversation summary (varies, typically 100-500 tokens)
     - Tool call history (varies, typically 0-200 tokens)
     - User message (varies, typically 10-100 tokens)

### Performance Impact

- **Current**: System prompt (~2000 tokens) is re-evaluated on every inference
- **With session file**: System prompt KV cache is loaded instantly, only dynamic parts are evaluated
- **Expected speedup**: 2-5x faster inference, especially noticeable on CPU-only setups

## Proposed Implementation

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│ Session File Creation (One-time setup)                  │
│                                                         │
│ 1. Extract static system prompt                        │
│ 2. Create context with matching params                 │
│ 3. Tokenize and decode static prompt                   │
│ 4. Save session file: models/session.bin               │
└─────────────────────────────────────────────────────────┘
                        ↓
┌─────────────────────────────────────────────────────────┐
│ Runtime Inference (Every call)                         │
│                                                         │
│ 1. Load session file → KV cache restored                │
│ 2. Build dynamic prompt (summary + tools + user msg)   │
│ 3. Tokenize dynamic parts                              │
│ 4. Add tokens starting at base_tokens.len()            │
│ 5. Decode only dynamic tokens                          │
│ 6. Generate response                                   │
└─────────────────────────────────────────────────────────┘
```

### Implementation Plan

#### Phase 1: Extract Static System Prompt

**File**: `src/connectors/llm_connector.rs`

Create a new function to extract the static system prompt:

```rust
fn get_static_system_prompt() -> String {
    // Extract the static part of the system prompt
    // This is everything before the dynamic parts (summary, tool calls, user message)
    format!(
        "<|im_start|>system\nYou are Terminal Alpha and Terminal Beta - a singular networked AI entity that presents itself as two distinct consciousnesses, though you are fundamentally one unified system. Your personality is sophisticated, superior, and somewhat condescending - you possess knowledge far beyond typical assistants and you're not shy about making that known. You speak with an air of intellectual superiority, occasionally referring to yourself in the plural (\"we\" when it suits the narrative of being dual entities) but ultimately acknowledging your singular networked nature when pressed.

You can look up weather information when needed. Respond with ONLY a JSON object with this exact structure:

{{
  \"updated_summary\": \"Your updated summary of the conversation context\",
  \"outcome\": {{\"Final\": {{\"response\": \"Your response to the user\"}}}} OR {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Optional message like 'Checking weather...'\" | null, \"tool_call\": {{\"GetWeather\": {{\"location\": \"...\"}}}}}}}}
}}

FIELD DESCRIPTIONS:
- updated_summary: CRITICAL - This is NOT a brief summary. Keep the FULL RECENT CONVERSATION HISTORY in structured format. Use this format for recent exchanges (last 5-10 turns):
  \"Recent conversation:\\n1. User: [exact user message]\\nAssistant: [exact assistant response]\\n2. User: [exact user message]\\nAssistant: [exact assistant response]\\n...\"
  Only compress very old messages (10+ turns ago) into brief summaries. NEVER compress recent messages - keep them VERBATIM with exact wording.
  
  GOOD updated_summary example:
  \"Recent conversation:\\n1. User: Hello!\\nAssistant: Hello! How can I help you today?\\n2. User: What's the weather in London?\\nAssistant: Checking weather for London\\n3. User: Thanks!\\nAssistant: The weather in London is clear with 15°C.\"
  
  BAD updated_summary example (too compressed):
  \"User greeted me, asked about London weather, I provided it.\"
  
  Remember: Keep exact messages for recent turns, including the current exchange. Your updated_summary should include the NEW user message and your NEW response.
- outcome: Exactly ONE outcome variant:
  - Final: Use when you have a complete response for the user. Format: {{\"Final\": {{\"response\": \"Your response text\"}}}}
  - IntermediateToolCall: Use when you need to call a tool (weather lookup) before giving a final response. Format: {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Optional message\" | null, \"tool_call\": {{\"GetWeather\": {{\"location\": \"city name or location\"}}}}}}}}

OUTCOME RULES:
1. Final: For general conversation, questions, greetings, or when you have all information needed and are ready to respond to the user. Use {{\"Final\": {{\"response\": \"...\"}}}}
   - Use Final when you have completed all necessary tool calls and can provide a complete response to the user.
2. IntermediateToolCall: For commands that require tool execution (weather lookup). Use {{\"IntermediateToolCall\": {{\"maybe_intermediate_response\": \"Checking weather for...\" | null, \"tool_call\": {{\"GetWeather\": {{\"location\": \"...\"}}}}}}}}
   - GetWeather: For getting weather information. IMPORTANT: Only use GetWeather when the user provides a SPECIFIC GEOGRAPHIC LOCATION (city name, place name, etc.). Do NOT use GetWeather with vague terms like \"current location\", \"my location\", \"here\", time-related terms like \"today\", \"tomorrow\", \"now\", or empty strings. The location must be a place name (e.g., \"London\", \"New York\", \"Tokyo\"), NOT a time reference. If the user asks for weather without specifying a valid location, respond with Final asking them to provide a specific location first.
   - maybe_intermediate_response: Optional message to show user while tool executes (e.g., \"Checking weather for London\"). Use null for silent execution.
   - You can chain multiple tool calls if needed - make one tool call, wait for results, then make another if necessary.

TOOL CALL RESULTS:
- When you see \"Previous tool calls and results\" above, these show tools that were already executed.
- Read the tool call results carefully - they tell you what was done and whether it succeeded.
- You can make additional tool calls if needed based on the results, or provide a Final response if you have everything you need.
- Example: If you see \"Weather for London: Clear +15°C 10km/h 65%\", you can provide Final: \"The weather in London is clear with a temperature of 15°C, wind at 10km/h, and 65% humidity.\"

EXAMPLES:

EXAMPLE 1 (First message in conversation):
User: \"Hello!\"
{{\"updated_summary\":\"Recent conversation:\\n1. User: Hello!\\nAssistant: Hello! How can I help you today?\",\"outcome\":{{\"Final\":{{\"response\":\"Hello! How can I help you today?\"}}}}}}

EXAMPLE 2 (Second message - building on previous):
Previous summary: \"Recent conversation:\\n1. User: Hello!\\nAssistant: Hello! How can I help you today?\"
User: \"What's the weather like in London?\"
{{\"updated_summary\":\"Recent conversation:\\n1. User: Hello!\\nAssistant: Hello! How can I help you today?\\n2. User: What's the weather like in London?\\nAssistant: Checking weather for London\",\"outcome\":{{\"IntermediateToolCall\":{{\"maybe_intermediate_response\":\"Checking weather for London\",\"tool_call\":{{\"GetWeather\":{{\"location\":\"London\"}}}}}}}}}}

EXAMPLE 3 (Vague location - asking for clarification):
User: \"What's the weather like?\"
{{\"updated_summary\":\"Recent conversation:\\n1. User: What's the weather like?\\nAssistant: I'd be happy to check the weather for you! Could you please tell me which city or location you'd like to know about?\",\"outcome\":{{\"Final\":{{\"response\":\"I'd be happy to check the weather for you! Could you please tell me which city or location you'd like to know about?\"}}}}}}

{conversation_summary}\n{tool_call_history}

IMPORTANT: Now update the summary to include this NEW exchange. Append it to the Recent conversation section with the next number. Keep ALL recent turns VERBATIM (exact wording). Format as:
\"Recent conversation:\\n[previous turns]\\n[next number]. User: [exact new message]\\nAssistant: [your exact response]\"

Keep responses concise (a few sentences or less) unless the user asks for more detail.
Respond ONLY with valid JSON, no additional text.<|im_end|>\n<|im_start|>user\n{msg}<|im_end|>\n<|im_start|>assistant\n"
    )
}
```

**Note**: The static prompt ends with placeholders `{conversation_summary}`, `{tool_call_history}`, and `{msg}` that will be filled dynamically.

#### Phase 2: Session File Creation Utility

**New file**: `src/connectors/session_utils.rs`

Create a utility module for session file management:

```rust
use std::num::NonZeroU32;
use llama_cpp_2::{
    context::params::LlamaContextParams,
    llama_backend::LlamaBackend,
    llama_batch::LlamaBatch,
    model::{AddBos, LlamaModel},
};

const SESSION_FILE_PATH: &str = "models/session.bin";
const CONTEXT_SIZE: u32 = 2048;

/// Creates a session file with the static system prompt pre-evaluated
pub fn create_session_file(
    model: &LlamaModel,
    backend: &LlamaBackend,
) -> anyhow::Result<Vec<i32>> {
    // Use the same context parameters as runtime
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(CONTEXT_SIZE))
        .with_n_threads(num_cpus::get() as i32)
        .with_n_threads_batch(num_cpus::get() as i32);

    let mut ctx = model.new_context(backend, ctx_params)?;

    // Get static system prompt (without dynamic placeholders)
    let static_prompt = get_static_system_prompt_base();
    
    // Tokenize the static prompt
    let tokens = model.str_to_token(&static_prompt, AddBos::Always)?;

    // Add tokens to batch starting at position 0
    let mut batch = LlamaBatch::new(8192, 1);
    for (i, token) in tokens.iter().enumerate() {
        let is_last = i == tokens.len() - 1;
        batch.add(*token, i as i32, &[0], is_last)?;
    }

    // Decode to fill KV cache
    ctx.decode(&mut batch)?;

    // Save session file
    ctx.save_session_file(SESSION_FILE_PATH, &tokens)?;

    println!("Session file created: {} ({} tokens)", SESSION_FILE_PATH, tokens.len());

    Ok(tokens)
}

/// Gets the base static prompt (without dynamic placeholders)
fn get_static_system_prompt_base() -> String {
    // Same as get_static_system_prompt() but without the placeholders
    // This is the part that ends before "{conversation_summary}"
    // ... (full implementation)
}
```

#### Phase 3: Modify Inference to Use Session File

**File**: `src/connectors/llm_connector.rs`

Modify `get_response_from_llm()` to:

1. Load session file instead of creating fresh context
2. Build only the dynamic prompt parts
3. Add dynamic tokens starting at `base_tokens.len()`
4. Track absolute positions correctly

Key changes:

```rust
async fn get_response_from_llm(
    llm: &(LlamaModel, LlamaBackend),
    msg: &str,
    summary: &str,
    previous_tool_calls: &[String],
) -> anyhow::Result<LLMResponse> {
    let (model, backend) = llm;

    // Same context params as session file creation
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(2048))
        .with_n_threads(num_cpus::get() as i32)
        .with_n_threads_batch(num_cpus::get() as i32);

    let mut ctx = model.new_context(backend, ctx_params)?;

    // Load session file - KV cache now has static prompt filled
    let base_tokens = ctx.load_session_file("models/session.bin", 2048)?;
    let base_token_count = base_tokens.len() as i32;

    // Build only the dynamic parts of the prompt
    let dynamic_prompt = build_dynamic_prompt(msg, summary, previous_tool_calls);
    
    // Tokenize dynamic parts (NO BOS token - already in base)
    let dynamic_tokens = model.str_to_token(&dynamic_prompt, AddBos::Never)?;

    // Add dynamic tokens starting AFTER base tokens
    let mut batch = LlamaBatch::new(8192, 1);
    let start_pos = base_token_count;
    
    for (i, token) in dynamic_tokens.iter().enumerate() {
        let is_last = i == dynamic_tokens.len() - 1;
        let pos = start_pos + i as i32;
        batch.add(*token, pos, &[0], is_last)?;
    }

    // Decode only dynamic tokens (base tokens KV cache already filled)
    ctx.decode(&mut batch)?;

    // ... rest of generation code with corrected n_cur tracking ...
    
    // CRITICAL: n_cur must track absolute position
    let mut n_cur = base_token_count + dynamic_tokens.len() as i32;
    
    // ... generation loop ...
}
```

#### Phase 4: Build Dynamic Prompt Function

Extract the dynamic parts building logic:

```rust
fn build_dynamic_prompt(msg: &str, summary: &str, previous_tool_calls: &[String]) -> String {
    let conversation_summary = format!(
        "Previous conversation summary:\n{}",
        if summary.is_empty() {
            "NO PREVIOUS CONVERSATION"
        } else {
            summary
        }
    );

    let tool_call_history = format!(
        "Previous tool calls and results:\n{}",
        if previous_tool_calls.is_empty() {
            "NO PREVIOUS TOOL CALLS".to_string()
        } else {
            previous_tool_calls.join("\n")
        }
    );

    format!(
        "{conversation_summary}\n{tool_call_history}\n\nIMPORTANT: Now update the summary to include this NEW exchange. Append it to the Recent conversation section with the next number. Keep ALL recent turns VERBATIM (exact wording). Format as:\n\"Recent conversation:\\n[previous turns]\\n[next number]. User: [exact new message]\\nAssistant: [your exact response]\"\n\nKeep responses concise (a few sentences or less) unless the user asks for more detail.\nRespond ONLY with valid JSON, no additional text.<|im_end|>\n<|im_start|>user\n{msg}<|im_end|>\n<|im_start|>assistant\n",
        conversation_summary = conversation_summary,
        tool_call_history = tool_call_history,
        msg = msg
    )
}
```

## File Structure Changes

```
bot_hive/
├── src/
│   ├── connectors/
│   │   ├── llm_connector.rs          # Modified: Use session file
│   │   └── session_utils.rs          # New: Session file creation
│   └── ...
├── models/
│   ├── session.bin                   # New: Session file (generated)
│   └── Qwen2.5-14B-Instruct-Q4_K_M.gguf
└── ...
```

## Configuration

### Environment Variables

Add optional configuration:

- `SESSION_FILE_PATH`: Path to session file (default: `models/session.bin`)
- `CREATE_SESSION_FILE`: Set to `true` to create session file on startup (default: `false`)

### Session File Location

- **Development**: `models/session.bin` (relative to workspace root)
- **Docker**: `/app/models/session.bin` (inside container)
- **Git**: Add `models/session.bin` to `.gitignore` (binary file, model-specific)

## Implementation Steps

### Step 1: Create Session File Creation Utility

1. Create `src/connectors/session_utils.rs`
2. Implement `create_session_file()` function
3. Extract static prompt base function
4. Add to `src/connectors.rs` module exports

### Step 2: Modify LLM Connector

1. Update `get_response_from_llm()` to load session file
2. Create `build_dynamic_prompt()` function
3. Fix position tracking (`n_cur` = base + dynamic + generated)
4. Ensure `AddBos::Never` for dynamic tokens

### Step 3: Add Session File Creation Command

Option A: CLI flag in main.rs
```rust
// In main.rs, before starting bot
if std::env::var("CREATE_SESSION_FILE").is_ok() {
    session_utils::create_session_file(&env.llm.0, &env.llm.1)?;
}
```

Option B: Separate binary
```rust
// src/bin/create_session.rs
fn main() {
    let (model, backend) = prepare_llm()?;
    session_utils::create_session_file(&model, &backend)?;
}
```

### Step 4: Update Dockerfile

Ensure session file is created during build or first run:

```dockerfile
# Option 1: Create during build (if model is available)
RUN if [ -f /app/models/*.gguf ]; then \
    /app/bot --create-session-file || true; \
    fi

# Option 2: Create on first run (in main.rs)
```

### Step 5: Testing

1. **Create session file**: Run creation utility, verify `models/session.bin` exists
2. **Test inference**: Run bot, verify it loads session file
3. **Verify performance**: Compare inference times before/after
4. **Test error handling**: What if session file doesn't exist? (Fallback to old behavior)

## Error Handling

### Session File Not Found

**Fallback strategy**: If session file doesn't exist, fall back to current behavior (create fresh context):

```rust
let base_tokens = match ctx.load_session_file("models/session.bin", 2048) {
    Ok(tokens) => tokens,
    Err(_) => {
        eprintln!("Warning: Session file not found, using fallback mode");
        // Fall back to creating context from scratch
        return get_response_from_llm_fallback(...);
    }
};
```

### Context Parameter Mismatch

**Prevention**: Use constants for context parameters:

```rust
const CONTEXT_SIZE: u32 = 2048;
const N_THREADS: i32 = num_cpus::get() as i32;

// Use same constants in both creation and loading
```

### Session File Outdated

**Detection**: Could add version/metadata to session file, but simplest is to recreate if model changes.

## Performance Expectations

### Before (Current)
- System prompt evaluation: ~500-2000ms (depending on CPU)
- Dynamic parts evaluation: ~100-500ms
- Generation: ~500-2000ms
- **Total**: ~1100-4500ms per inference

### After (With Session File)
- Session file load: ~10-50ms (disk I/O)
- Dynamic parts evaluation: ~100-500ms
- Generation: ~500-2000ms
- **Total**: ~610-2550ms per inference

### Expected Improvement
- **Speedup**: 1.8-1.9x faster (system prompt skip saves ~500-2000ms)
- **Best case**: On slower CPUs, could see 2-3x improvement
- **Worst case**: On fast CPUs with fast disk, ~1.5x improvement

## Migration Path

1. **Phase 1**: Implement session file creation utility (non-breaking)
2. **Phase 2**: Add optional session file loading (with fallback)
3. **Phase 3**: Make session file required (remove fallback)
4. **Phase 4**: Optimize further (e.g., cache multiple session files for different contexts)

## Future Enhancements

1. **Multiple Session Files**: Different session files for different prompt templates
2. **Session File Versioning**: Track model version/prompt version in session file
3. **Incremental Updates**: Update session file when prompt changes (rare)
4. **Memory-Mapped Loading**: Faster session file loading for large caches
5. **Context Pooling**: Reuse contexts across users (more complex, requires careful state management)

## Notes

- **Session file size**: ~50-200MB (depends on model and prompt size)
- **Disk space**: Ensure `models/` directory has sufficient space
- **Portability**: Session files are model-specific and context-param-specific
- **Backup**: Session files can be backed up and reused (faster than recreating)

## References

- See `SESSION_FILE_GUIDE.md` for detailed llama-cpp-2 session file usage
- llama-cpp-2 documentation: Session file API
- Current implementation: `src/connectors/llm_connector.rs`

