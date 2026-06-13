use crate::{Effects, EntityId, Identified, Scheduled, StateMachine, StateMachineHandle};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration as StdDuration;

impl EntityId for String {
    fn get_id_string(&self) -> String {
        self.clone()
    }
}

#[derive(Default)]
struct Obs {
    totals: Vec<i64>,
    ticks: u32,
}

struct CounterEnv {
    obs: Arc<Mutex<Obs>>,
}

static COUNTER: OnceLock<StateMachineHandle<CounterMachine>> = OnceLock::new();

struct CounterMachine;

#[derive(Clone, Serialize, Deserialize)]
struct CounterState {
    total: i64,
    tick_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
enum CounterAction {
    Add(i64),
    Ping,
    Tick,
}

struct CounterInit {
    id: String,
    start: i64,
}

impl Identified for CounterInit {
    type Id = String;
    fn get_id(&self) -> &String {
        &self.id
    }
}

impl StateMachine for CounterMachine {
    type State = CounterState;
    type Id = String;
    type Action = CounterAction;
    type Construction = CounterInit;
    type Env = CounterEnv;

    fn construct(init: CounterInit, _effects: &mut Effects<Self>) -> CounterState {
        CounterState {
            total: init.start,
            tick_at: Some(Utc::now() + Duration::milliseconds(50)),
        }
    }

    fn transition(
        state: &CounterState,
        id: &String,
        env: &Arc<CounterEnv>,
        action: &CounterAction,
        effects: &mut Effects<Self>,
    ) -> anyhow::Result<CounterState> {
        match action {
            CounterAction::Add(n) => {
                if state.total + n < 0 {
                    anyhow::bail!("would go negative");
                }
                let mut next = state.clone();
                next.total += n;
                env.obs.lock().unwrap().totals.push(next.total);
                Ok(next)
            }
            CounterAction::Ping => {
                effects.enqueue_action::<CounterMachine>(id.clone(), CounterAction::Add(10));
                Ok(state.clone())
            }
            CounterAction::Tick => {
                let mut next = state.clone();
                next.tick_at = None;
                env.obs.lock().unwrap().ticks += 1;
                Ok(next)
            }
        }
    }

    fn schedule(state: &CounterState) -> Option<Scheduled<CounterAction>> {
        state.tick_at.map(|at| Scheduled {
            at,
            action: CounterAction::Tick,
        })
    }

    fn handle() -> &'static StateMachineHandle<CounterMachine> {
        COUNTER.get().expect("CounterMachine not initialized")
    }
}

#[tokio::test]
async fn smoke() {
    let obs = Arc::new(Mutex::new(Obs::default()));
    COUNTER
        .set(StateMachineHandle::<CounterMachine>::new(CounterEnv { obs: obs.clone() }))
        .ok()
        .expect("set COUNTER once");
    let sm = CounterMachine::handle();

    sm.maybe_construct(CounterInit { id: "c1".to_string(), start: 0 }).await;
    sm.act("c1".to_string(), CounterAction::Add(5)).await;

    sm.maybe_construct(CounterInit { id: "c1".to_string(), start: 999 }).await;
    sm.act("c1".to_string(), CounterAction::Add(3)).await;

    sm.act("c1".to_string(), CounterAction::Add(-1000)).await;
    sm.act("c1".to_string(), CounterAction::Ping).await;

    tokio::time::sleep(StdDuration::from_millis(120)).await;

    {
        let o = obs.lock().unwrap();
        assert_eq!(o.totals, vec![5, 8, 18], "idempotency, Err no-op, and loop-back");
        assert_eq!(o.ticks, 1, "timer fired exactly once");
    }

    sm.delete("c1".to_string()).await;
    tokio::time::sleep(StdDuration::from_millis(20)).await;
    sm.act("c1".to_string(), CounterAction::Add(1)).await;
    tokio::time::sleep(StdDuration::from_millis(20)).await;

    assert_eq!(obs.lock().unwrap().totals, vec![5, 8, 18], "post-delete act dropped");
}

struct RtEnv {
    received: Arc<Mutex<Vec<i64>>>,
}

static PONGER: OnceLock<StateMachineHandle<PongerMachine>> = OnceLock::new();
static PINGER: OnceLock<StateMachineHandle<PingerMachine>> = OnceLock::new();

struct PongerMachine;
#[derive(Clone, Serialize, Deserialize)]
struct PongerState;
#[derive(Debug, Serialize, Deserialize)]
enum PongerAction {
    Pong(i64),
}
struct PongerInit {
    id: String,
}
impl Identified for PongerInit {
    type Id = String;
    fn get_id(&self) -> &String {
        &self.id
    }
}

impl StateMachine for PongerMachine {
    type State = PongerState;
    type Id = String;
    type Action = PongerAction;
    type Construction = PongerInit;
    type Env = RtEnv;
    fn construct(_init: PongerInit, _effects: &mut Effects<Self>) -> PongerState {
        PongerState
    }
    fn transition(
        state: &PongerState,
        _id: &String,
        env: &Arc<RtEnv>,
        action: &PongerAction,
        _effects: &mut Effects<Self>,
    ) -> anyhow::Result<PongerState> {
        let PongerAction::Pong(n) = action;
        env.received.lock().unwrap().push(*n);
        Ok(state.clone())
    }
    fn schedule(_state: &PongerState) -> Option<Scheduled<PongerAction>> {
        None
    }
    fn handle() -> &'static StateMachineHandle<PongerMachine> {
        PONGER.get().expect("PongerMachine not initialized")
    }
}

struct PingerMachine;
#[derive(Clone, Serialize, Deserialize)]
struct PingerState;
#[derive(Debug, Serialize, Deserialize)]
enum PingerAction {
    Ping(i64),
}
struct PingerInit {
    id: String,
}
impl Identified for PingerInit {
    type Id = String;
    fn get_id(&self) -> &String {
        &self.id
    }
}

impl StateMachine for PingerMachine {
    type State = PingerState;
    type Id = String;
    type Action = PingerAction;
    type Construction = PingerInit;
    type Env = RtEnv;
    fn construct(_init: PingerInit, effects: &mut Effects<Self>) -> PingerState {
        effects.enqueue_action::<PongerMachine>("pong1".to_string(), PongerAction::Pong(0));
        PingerState
    }
    fn transition(
        state: &PingerState,
        _id: &String,
        _env: &Arc<RtEnv>,
        action: &PingerAction,
        effects: &mut Effects<Self>,
    ) -> anyhow::Result<PingerState> {
        let PingerAction::Ping(n) = action;
        if *n < 0 {
            anyhow::bail!("no negative pings");
        }
        effects.enqueue_action::<PongerMachine>("pong1".to_string(), PongerAction::Pong(*n));
        Ok(state.clone())
    }
    fn schedule(_state: &PingerState) -> Option<Scheduled<PingerAction>> {
        None
    }
    fn handle() -> &'static StateMachineHandle<PingerMachine> {
        PINGER.get().expect("PingerMachine not initialized")
    }
}

#[tokio::test]
async fn outbound() {
    let received = Arc::new(Mutex::new(Vec::<i64>::new()));
    PONGER
        .set(StateMachineHandle::<PongerMachine>::new(RtEnv { received: received.clone() }))
        .ok()
        .expect("set PONGER once");
    PINGER
        .set(StateMachineHandle::<PingerMachine>::new(RtEnv { received: received.clone() }))
        .ok()
        .expect("set PINGER once");
    let ponger = PongerMachine::handle();
    let pinger = PingerMachine::handle();

    ponger.maybe_construct(PongerInit { id: "pong1".to_string() }).await;
    pinger.maybe_construct(PingerInit { id: "ping1".to_string() }).await;

    pinger.act("ping1".to_string(), PingerAction::Ping(42)).await;
    pinger.act("ping1".to_string(), PingerAction::Ping(-1)).await;

    tokio::time::sleep(StdDuration::from_millis(50)).await;
    assert_eq!(
        *received.lock().unwrap(),
        vec![0, 42],
        "construct effect fired on creation (0); Ping(42) committed; Ping(-1) errored so no outbound"
    );
}
