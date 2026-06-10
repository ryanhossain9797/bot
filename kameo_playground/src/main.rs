// What a domain user of the framework writes: pure state types, each with its OWN Env type, a
// StateMachine impl, one `entity!` line, and calls to construct/act. The framework never names or
// provides Env — each state machine declares its own associated Env.
mod framework;

use framework::{act, bootstrap, construct, Effects, Env, EntityId, Scheduled, StateMachine};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ===================== entity: Convo =====================

pub struct ConvoId(pub String);

impl EntityId for ConvoId {
    fn id_string(&self) -> String {
        self.0.clone()
    }
}

pub struct Convo {
    id: ConvoId,
    count: i64,
    tick_armed: bool,
}

#[derive(Serialize, Deserialize)]
pub enum ConvoAction {
    Say(String),
    Tick,
}

pub struct ConvoConstruction;

// Convo's own env — implements the framework's Env trait; the framework never names this type.
pub struct ConvoEnv {
    greeting: String,
}

impl Env for ConvoEnv {
    fn get_config(&self) {
        println!("[env] greeting={}", self.greeting);
    }
}

impl StateMachine for Convo {
    type Id = ConvoId;
    type Action = ConvoAction;
    type Construction = ConvoConstruction;
    type Env = ConvoEnv;
    type Wrapped = ConvoEntity;
    fn build_env() -> anyhow::Result<ConvoEnv> {
        Ok(ConvoEnv {
            greeting: "TerminalAlphaBeta".to_string(),
        })
    }
    fn construct(id: ConvoId, _construction: ConvoConstruction) -> Self {
        Convo {
            id,
            count: 0,
            tick_armed: false,
        }
    }
    fn id(&self) -> &ConvoId {
        &self.id
    }
    fn transition(&mut self, env: &dyn Env, action: ConvoAction) -> Effects<Self> {
        env.get_config();
        match action {
            ConvoAction::Say(s) => {
                self.count += 1;
                self.tick_armed = true; // schedule() will now yield a Tick on a timer
                println!("[{}] '{s}' -> count={}", self.id.id_string(), self.count);
                // an outbound message to the Counter entity (the Tick comes from the timer)
                Effects::none()
                    .send::<Counter>(CounterId("counter:1".to_string()), CounterAction::Add(1))
            }
            ConvoAction::Tick => {
                self.tick_armed = false; // schedule() now yields None — timer stops
                println!("[{}] scheduled tick fired", self.id.id_string());
                Effects::none()
            }
        }
    }
    fn schedule(&self) -> Option<Scheduled<ConvoAction>> {
        self.tick_armed.then(|| Scheduled {
            after: Duration::from_millis(150),
            action: ConvoAction::Tick,
        })
    }
}

entity!(ConvoEntity, Convo, "convo-action");

// ===================== entity: Counter =====================

pub struct CounterId(pub String);

impl EntityId for CounterId {
    fn id_string(&self) -> String {
        self.0.clone()
    }
}

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

// Counter's own env — a *different* type from ConvoEnv, also implementing Env.
pub struct CounterEnv {
    tag: String,
}

impl Env for CounterEnv {
    fn get_config(&self) {
        println!("[env] tag={}", self.tag);
    }
}

impl StateMachine for Counter {
    type Id = CounterId;
    type Action = CounterAction;
    type Construction = CounterInit;
    type Env = CounterEnv;
    type Wrapped = CounterEntity;
    fn build_env() -> anyhow::Result<CounterEnv> {
        Ok(CounterEnv {
            tag: "CTR".to_string(),
        })
    }
    fn construct(id: CounterId, init: CounterInit) -> Self {
        Counter {
            id,
            total: init.start,
        }
    }
    fn id(&self) -> &CounterId {
        &self.id
    }
    fn transition(&mut self, env: &dyn Env, action: CounterAction) -> Effects<Self> {
        env.get_config();
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
        Effects::none()
    }
    fn schedule(&self) -> Option<Scheduled<CounterAction>> {
        None
    }
}

entity!(CounterEntity, Counter, "counter-action");

// ===================== driver =====================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap()?; // swarm
    Convo::bootstrap()?; // register Convo's env
    Counter::bootstrap()?; // register Counter's env

    // Construct the Counter first so it exists when Convo messages it.
    construct::<Counter>(CounterId("counter:1".to_string()), CounterInit { start: 10 }).await?;
    construct::<Convo>(ConvoId("convo:1".to_string()), ConvoConstruction).await?;

    // One Convo message → fans out: a self-action (Tick) back to Convo + an outbound Add(1) to Counter.
    act::<Convo>("convo:1", ConvoAction::Say("hello".to_string())).await?;

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    Ok(())
}
