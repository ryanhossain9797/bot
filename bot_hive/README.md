# User Lifecycle Documentation

## Overview

This document describes the user lifecycle and state management system in the bot. The system implements a type-safe state machine that manages user interactions through distinct states and transitions.

## User States

The user lifecycle consists of four states defined in `bot_hive/src/models/user.rs:6-13`:

### 1. Idle (Default State)
- The initial state when a user is not interacting with the bot
- Users return to this state after completing a conversation cycle
- Ready to receive new messages

### 2. RespondingToMessage
- Entered when a user sends a message that starts a conversation
- The bot processes the message and sends a response
- Transitions to WaitingToSayGoodbye after response is sent

### 3. WaitingToSayGoodbye
- Entered after the bot responds to a user's message
- Contains a scheduled timeout (currently 10 seconds)
- Allows a delay before sending the goodbye message
- Transitions to SayingGoodbye when timeout expires

### 4. SayingGoodbye
- The bot sends a goodbye message to the user
- Final state before returning to Idle
- Transitions back to Idle after message is sent

## User Actions

Actions trigger state transitions (defined in `bot_hive/src/models/user.rs:15-21`):

### NewMessage
```rust
NewMessage {
    msg: String,
    start_conversation: bool,
}
```
- Triggered when user sends a message
- `start_conversation: true` initiates a new conversation from Idle state

### Timeout
- Triggered by scheduled timeout events
- Used to trigger the goodbye message after waiting period

### SendResult
```rust
SendResult(Arc<anyhow::Result<()>>)
```
- Indicates the result of sending a message to the user
- Triggers state transitions after message operations complete

## State Transitions

The complete lifecycle flow (`bot_hive/src/life_cycles/user_life_cycle.rs:8-53`):

```
         START
          |
          v
       [IDLE] (Default state)
          |
          | NewMessage { start_conversation: true }
          |
          v
  [RESPONDING TO MESSAGE]
          |
          | SendResult(Ok) after bot response
          | (schedules 10s timeout)
          |
          v
[WAITING TO SAY GOODBYE]
          |
          | Timeout (after 10s)
          |
          v
    [SAYING GOODBYE]
          |
          | SendResult(Ok) after goodbye message
          |
          v
       [IDLE]
          |
          +--- (Cycle repeats)
```

### Transition 1: Idle → RespondingToMessage
**Trigger:** User sends a message with `start_conversation: true`

**Implementation:** `bot_hive/src/life_cycles/user_life_cycle.rs:12-24`

**Actions:**
- Bot processes the message
- Schedules external operation to send response
- Moves to RespondingToMessage state

### Transition 2: RespondingToMessage → WaitingToSayGoodbye
**Trigger:** Bot successfully sends the response message (SendResult)

**Implementation:** `bot_hive/src/life_cycles/user_life_cycle.rs:25-32`

**Actions:**
- Sets timeout to 10 seconds from now
- Moves to WaitingToSayGoodbye state
- Timeout is automatically scheduled by the framework

### Transition 3: WaitingToSayGoodbye → SayingGoodbye
**Trigger:** Timeout expires (10 seconds after response)

**Implementation:** `bot_hive/src/life_cycles/user_life_cycle.rs:33-42`

**Actions:**
- Schedules external operation to send goodbye message
- Moves to SayingGoodbye state

### Transition 4: SayingGoodbye → Idle
**Trigger:** Goodbye message successfully sent (SendResult)

**Implementation:** `bot_hive/src/life_cycles/user_life_cycle.rs:43-47`

**Actions:**
- Returns to Idle state
- Ready for next conversation
- No scheduled events

## Scheduling System

The scheduling function (`bot_hive/src/life_cycles/user_life_cycle.rs:55-65`) determines when timeouts should occur:

```rust
pub fn schedule(user: &User) -> Vec<Scheduled<UserAction>> {
    match user.state {
        UserState::WaitingToSayGoodbye(Some(timeout)) => {
            vec![Scheduled {
                at: timeout,
                action: UserAction::Timeout,
            }]
        }
        _ => Vec::new(),
    }
}
```

Only the `WaitingToSayGoodbye` state generates scheduled events. The framework automatically manages these timeouts.

## Architecture

### State Machine Framework

The system uses a generic state machine implementation (`lib_hive/src/lib.rs`) with:

- **Type-safe transitions:** Invalid state-action combinations are caught at compile time
- **Async-first design:** All transitions and operations are asynchronous
- **Channel-based communication:** Uses tokio mpsc channels for thread-safe state management
- **External operations:** Side effects (like sending messages) are separated from state logic
- **Per-user isolation:** Each user has an independent lifecycle instance

### Integration

The lifecycle is integrated in `bot_hive/src/main.rs:34-38`:

```rust
let user_life_cycle = new_life_cycle(
    env, 
    Transition(user_transition), 
    Schedule(schedule)
);
```

This creates a handle that processes user actions through the state machine.

## Error Handling

Invalid state-action combinations return an error:

```rust
_ => Err(anyhow::anyhow!("Invalid state or action"))
```

This ensures:
- Messages in wrong states are rejected
- Timeouts only fire in appropriate states
- SendResult only accepted after send operations

## Key Features

1. **Deterministic behavior:** Each state-action combination has exactly one outcome
2. **Automatic timeout management:** Framework handles scheduling and timeout delivery
3. **Clean separation of concerns:** State logic separate from message sending
4. **Concurrent safety:** Multiple users can interact independently
5. **Type safety:** Rust's type system prevents many bugs at compile time

## Configuration

Current timeout configuration (in `bot_hive/src/life_cycles/user_life_cycle.rs:28`):
- Goodbye delay: 10 seconds (10,000 milliseconds)

To modify the goodbye delay, change the `ChronoDuration::milliseconds(10_000)` value in the RespondingToMessage → WaitingToSayGoodbye transition.

## Example Flow

1. User sends "Hello" on Discord
2. Discord connector creates `NewMessage` action
3. State: Idle → RespondingToMessage
4. Bot sends response "Hi there!"
5. State: RespondingToMessage → WaitingToSayGoodbye(timeout in 10s)
6. (10 seconds pass)
7. Timeout fires
8. State: WaitingToSayGoodbye → SayingGoodbye
9. Bot sends "Goodbye"
10. State: SayingGoodbye → Idle
11. Ready for next conversation
