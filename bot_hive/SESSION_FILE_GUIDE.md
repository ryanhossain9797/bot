# Session File Save/Load Guide for llama-cpp-2

## Overview

Session files allow you to persist the KV cache (key-value cache) of a pre-processed prompt, avoiding expensive re-evaluation on subsequent runs. This is especially useful for large system prompts that don't change between inference calls.

## Saving a Session File

### Process

1. **Create a fresh context** with the same parameters you'll use when loading
2. **Tokenize your base prompt** (e.g., system prompt)
3. **Add tokens to batch** starting at position 0
4. **Decode the batch** to fill the KV cache
5. **Save the session file** with the tokens

### Code Pattern

```rust
// 1. Create context
let ctx_params = LlamaContextParams::default()
    .with_n_ctx(NonZeroU32::new(2048))
    .with_n_threads(num_cpus::get() as i32)
    .with_n_threads_batch(num_cpus::get() as i32);
let mut ctx = model.new_context(backend, ctx_params)?;

// 2. Tokenize
let tokens = model.str_to_token(system_prompt, AddBos::Always)?;

// 3. Add to batch (positions 0, 1, 2, ...)
let mut batch = LlamaBatch::new(8192, 1);
for (i, token) in tokens.iter().enumerate() {
    let is_last = i == tokens.len() - 1;
    batch.add(*token, i as i32, &[0], is_last)?;
}

// 4. Decode to fill KV cache
ctx.decode(&mut batch)?;

// 5. Save session file
ctx.save_session_file(SESSION_FILE, &tokens)?;
```

### Important Notes

- **Context parameters must match** between save and load (same `n_ctx`, `n_threads`, etc.)
- **Save the tokens** that were used to create the KV cache
- The session file contains both the KV cache state AND the token sequence

## Loading a Session File

### Process

1. **Create a fresh context** with the SAME parameters used when saving
2. **Load the session file** - this restores the KV cache
3. **DO NOT re-decode the base tokens** - the KV cache is already filled!
4. **Add new tokens** starting at position `base_tokens.len()`
5. **Continue inference** from there

### Code Pattern

```rust
// 1. Create context (same params as save)
let ctx_params = LlamaContextParams::default()
    .with_n_ctx(NonZeroU32::new(2048))
    .with_n_threads(num_cpus::get() as i32)
    .with_n_threads_batch(num_cpus::get() as i32);
let mut ctx = model.new_context(backend, ctx_params)?;

// 2. Load session file (restores KV cache)
let base_tokens = ctx.load_session_file(session_path, 2048)?;
// KV cache now has positions 0 through base_tokens.len() - 1 filled

// 3. Tokenize new prompt
let user_tokens = model.str_to_token(&user_prompt, AddBos::Never)?;

// 4. Add new tokens starting AFTER base tokens
let mut batch = LlamaBatch::new(8192, 1);
let start_pos = base_tokens.len() as i32;  // Critical: start after base tokens

for (i, token) in user_tokens.iter().enumerate() {
    let is_last = i == user_tokens.len() - 1;
    let pos = start_pos + i as i32;  // Position continues from base_tokens
    batch.add(*token, pos, &[0], is_last)?;
}

// 5. Decode new tokens (base tokens KV cache already filled)
ctx.decode(&mut batch)?;
```

## Critical Gotchas

### 1. **DO NOT Re-decode Base Tokens After Loading**

**WRONG:**
```rust
let base_tokens = ctx.load_session_file(session_path, 2048)?;
// DON'T DO THIS - KV cache already has positions 0-1075 filled!
for (i, token) in base_tokens.iter().enumerate() {
    batch.add(*token, i as i32, &[0], false)?;  // ERROR: position mismatch!
}
ctx.decode(&mut batch)?;  // Error: KV cache already has position 1075, you're trying to add position 0
```

**RIGHT:**
```rust
let base_tokens = ctx.load_session_file(session_path, 2048)?;
// KV cache is already filled - skip re-evaluation
// Go straight to adding new tokens at start_pos = base_tokens.len()
```

**Why:** `load_session_file()` already restores the KV cache. Re-decoding would try to add tokens at positions 0-N when the KV cache already has positions 0-N filled, causing a position mismatch error.

### 2. **Position Tracking: `n_cur` Must Track Absolute Position**

**WRONG:**
```rust
let mut n_cur = batch.n_tokens();  // Only counts user prompt tokens!
// If base_tokens.len() = 1076 and user_tokens.len() = 10
// n_cur = 10, but should be 1086!
```

**RIGHT:**
```rust
let mut n_cur = (base_tokens.len() + user_tokens.len()) as i32;
// n_cur = 1076 + 10 = 1086 (absolute position in sequence)
```

**Why:** When generating tokens, you need to add them at the correct absolute position in the sequence. The batch only contains the most recent tokens, but positions must be consecutive across the entire sequence (base + user + generated).

### 3. **Start Position for New Tokens**

**CRITICAL:** New tokens must start at `base_tokens.len()`, not 0.

```rust
let start_pos = base_tokens.len() as i32;  // e.g., 1076
// Add user tokens at positions 1076, 1077, 1078, ...
```

**Why:** llama.cpp requires consecutive positions. If KV cache has 0-1075 filled, the next batch must start at 1076.

### 4. **Context Parameters Must Match**

The context parameters used when loading must match those used when saving:

```rust
// When saving:
.with_n_ctx(NonZeroU32::new(2048))

// When loading:
.with_n_ctx(NonZeroU32::new(2048))  // MUST match!
```

**Why:** KV cache size depends on context size. Mismatched parameters can cause errors or incorrect behavior.

### 5. **max_tokens Parameter in load_session_file**

```rust
let base_tokens = ctx.load_session_file(session_path, 2048)?;
//                                                      ^^^^
// This should match or exceed your context size
```

**Why:** This allocates buffer space for the returned tokens. If the session file has more tokens than this, it will error.

## Error Messages to Watch For

### "inconsistent sequence positions: X = 1075, Y = 0"

**Cause:** Trying to re-decode base tokens after loading session file.

**Fix:** Skip re-decoding base tokens. The KV cache is already filled.

### "n_tokens == 0" decode error

**Cause:** Position mismatch - usually from incorrect `n_cur` or `start_pos` calculation.

**Fix:** Ensure positions are consecutive and absolute (not relative to batch).

## Complete Example Flow

```rust
// SAVE (one-time setup)
fn save_session() {
    let mut ctx = model.new_context(backend, ctx_params)?;
    let tokens = model.str_to_token(system_prompt, AddBos::Always)?;
    
    let mut batch = LlamaBatch::new(8192, 1);
    for (i, token) in tokens.iter().enumerate() {
        batch.add(*token, i as i32, &[0], i == tokens.len() - 1)?;
    }
    ctx.decode(&mut batch)?;
    ctx.save_session_file("session.bin", &tokens)?;
}

// LOAD (every inference)
fn load_and_infer() {
    let mut ctx = model.new_context(backend, ctx_params)?;  // Same params!
    
    // Load session - KV cache now filled with positions 0..N-1
    let base_tokens = ctx.load_session_file("session.bin", 2048)?;
    
    // Add new tokens starting at position N
    let user_tokens = model.str_to_token(user_prompt, AddBos::Never)?;
    let mut batch = LlamaBatch::new(8192, 1);
    let start_pos = base_tokens.len() as i32;
    
    for (i, token) in user_tokens.iter().enumerate() {
        batch.add(*token, start_pos + i as i32, &[0], i == user_tokens.len() - 1)?;
    }
    ctx.decode(&mut batch)?;
    
    // Generate - track absolute position
    let mut n_cur = (base_tokens.len() + user_tokens.len()) as i32;
    loop {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        // ... handle token ...
        
        batch.clear();
        batch.add(token, n_cur, &[0], true)?;  // Absolute position!
        n_cur += 1;
        ctx.decode(&mut batch)?;
    }
}
```

## Key Takeaways

1. **Session files save KV cache state** - no need to re-evaluate base prompt
2. **Never re-decode base tokens** after loading - KV cache already filled
3. **Track absolute positions** - `n_cur` = base + user + generated lengths
4. **New tokens start at `base_tokens.len()`** - maintain consecutive positions
5. **Context parameters must match** between save and load
6. **The documentation is misleading** - "pass tokens to context" means track them, not re-decode them

