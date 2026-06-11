// What a domain user of the framework writes: pure state types (each with its OWN Env), a
// StateMachine impl, and calls to act_maybe_construct / act / delete. No macros, no concrete entity
// types — the framework's single generic wrapper is the actor for every state machine.
mod framework;

use chrono::{DateTime, Utc};
use framework::{
    act, act_maybe_construct, construct, delete, register_env, Effects, EntityId, Scheduled,
    StateMachine,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

// ===================== entity: Convo =====================

#[derive(Clone, Serialize, Deserialize)]
pub struct ConvoId(pub String);

impl EntityId for ConvoId {
    fn id_string(&self) -> String {
        self.0.clone()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Convo {
    id: ConvoId,
    count: i64,
    // Absolute deadline for the next tick, stored in state so schedule() returns a stable instant.
    tick_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize)]
pub enum ConvoAction {
    Say(String),
    Tick,
}

pub struct ConvoConstruction;

// Convo's own env — a plain type; the framework only needs Send + Sync + 'static to store it.
pub struct ConvoEnv {
    greeting: String,
}

impl StateMachine for Convo {
    type Id = ConvoId;
    type Action = ConvoAction;
    type Construction = ConvoConstruction;
    type Env = ConvoEnv;
    fn construct(id: ConvoId, _construction: ConvoConstruction) -> Self {
        Convo {
            id,
            count: 0,
            tick_at: None,
        }
    }
    fn id(&self) -> &ConvoId {
        &self.id
    }
    fn transition(
        mut self,
        env: Arc<ConvoEnv>,
        action: &ConvoAction,
    ) -> anyhow::Result<(Self, Effects<Self>)> {
        println!("[env] greeting={}", env.greeting);
        match action {
            ConvoAction::Say(s) => {
                self.count += 1;
                self.tick_at = Some(Utc::now() + chrono::Duration::milliseconds(150));
                println!("[{}] '{s}' -> count={}", self.id.id_string(), self.count);
                let effects = Effects::none()
                    .send::<Counter>(CounterId("counter:1".to_string()), CounterAction::Add(1));
                Ok((self, effects))
            }
            ConvoAction::Tick => {
                self.tick_at = None;
                println!("[{}] scheduled tick fired", self.id.id_string());
                Ok((self, Effects::none()))
            }
        }
    }
    fn schedule(&self) -> Option<Scheduled<ConvoAction>> {
        self.tick_at.map(|at| Scheduled {
            at,
            action: ConvoAction::Tick,
        })
    }
}

// ===================== entity: Counter =====================

#[derive(Clone, Serialize, Deserialize)]
pub struct CounterId(pub String);

impl EntityId for CounterId {
    fn id_string(&self) -> String {
        self.0.clone()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Counter {
    id: CounterId,
    total: i64,
}

#[derive(Serialize, Deserialize)]
pub enum CounterAction {
    Add(i64),
    Reset,
}

pub struct CounterInit {
    pub start: i64,
}

pub struct CounterEnv {
    tag: String,
}

impl StateMachine for Counter {
    type Id = CounterId;
    type Action = CounterAction;
    type Construction = CounterInit;
    type Env = CounterEnv;
    fn construct(id: CounterId, init: CounterInit) -> Self {
        Counter {
            id,
            total: init.start,
        }
    }
    fn id(&self) -> &CounterId {
        &self.id
    }
    fn transition(
        mut self,
        env: Arc<CounterEnv>,
        action: &CounterAction,
    ) -> anyhow::Result<(Self, Effects<Self>)> {
        println!("[env] tag={}", env.tag);
        match action {
            CounterAction::Add(n) => {
                self.total += n;
                println!("[{}] +{n} -> total={}", self.id.id_string(), self.total);
            }
            CounterAction::Reset => {
                self.total = 0;
                println!("[{}] reset -> total=0", self.id.id_string());
            }
        }
        Ok((self, Effects::none()))
    }
    fn schedule(&self) -> Option<Scheduled<CounterAction>> {
        None
    }
}

// ===================== driver =====================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // main provides each state machine's env at startup — the framework builds none of its own.
    // (Any async setup a real env needs — loading a model, opening a DB — happens here, in main.)
    register_env::<Convo>(ConvoEnv {
        greeting: "TerminalAlphaBeta".to_string(),
    });
    register_env::<Counter>(CounterEnv {
        tag: "CTR".to_string(),
    });

    construct::<Counter>(CounterId("counter:1".to_string()), CounterInit { start: 10 })?;

    // First message: act_maybe_construct creates convo:1, then acts (count -> 1).
    act_maybe_construct::<Convo>(
        ConvoId("convo:1".to_string()),
        ConvoConstruction,
        ConvoAction::Say("hello".to_string()),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Second message: maybe_construct finds the existing entity and just acts (count -> 2, NOT reset
    // to a fresh 0). This is the idempotency fix — a re-construct does not overwrite live state.
    act_maybe_construct::<Convo>(
        ConvoId("convo:1".to_string()),
        ConvoConstruction,
        ConvoAction::Say("world".to_string()),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Framework meta-action: tear the Counter down, then prove a later domain action misses cleanly.
    delete::<Counter>("counter:1").await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    act::<Counter>("counter:1", CounterAction::Add(1)).await?;

    Ok(())
}
