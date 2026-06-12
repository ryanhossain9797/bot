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
    id: String,
    total: i64,
    tick_at: Option<DateTime<Utc>>,
}

impl Identified for CounterState {
    type Id = String;
    fn get_id(&self) -> &String {
        &self.id
    }
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

    fn construct(init: CounterInit) -> (CounterState, Effects<Self>) {
        (
            CounterState {
                id: init.id,
                total: init.start,
                tick_at: Some(Utc::now() + Duration::milliseconds(50)),
            },
            Effects::none(),
        )
    }

    fn transition(
        state: &CounterState,
        env: &Arc<CounterEnv>,
        action: &CounterAction,
    ) -> anyhow::Result<(CounterState, Effects<Self>)> {
        match action {
            CounterAction::Add(n) => {
                if state.total + n < 0 {
                    anyhow::bail!("would go negative");
                }
                let mut next = state.clone();
                next.total += n;
                env.obs.lock().unwrap().totals.push(next.total);
                Ok((next, Effects::none()))
            }
            CounterAction::Ping => {
                Ok((state.clone(), Effects::none().then(async { CounterAction::Add(10) })))
            }
            CounterAction::Tick => {
                let mut next = state.clone();
                next.tick_at = None;
                env.obs.lock().unwrap().ticks += 1;
                Ok((next, Effects::none()))
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
struct PongerState {
    id: String,
}
impl Identified for PongerState {
    type Id = String;
    fn get_id(&self) -> &String {
        &self.id
    }
}
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
    fn construct(init: PongerInit) -> (PongerState, Effects<Self>) {
        (PongerState { id: init.id }, Effects::none())
    }
    fn transition(
        state: &PongerState,
        env: &Arc<RtEnv>,
        action: &PongerAction,
    ) -> anyhow::Result<(PongerState, Effects<Self>)> {
        let PongerAction::Pong(n) = action;
        env.received.lock().unwrap().push(*n);
        Ok((state.clone(), Effects::none()))
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
struct PingerState {
    id: String,
}
impl Identified for PingerState {
    type Id = String;
    fn get_id(&self) -> &String {
        &self.id
    }
}
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
    fn construct(init: PingerInit) -> (PingerState, Effects<Self>) {
        (
            PingerState { id: init.id },
            Effects::none().send::<PongerMachine>("pong1".to_string(), PongerAction::Pong(0)),
        )
    }
    fn transition(
        state: &PingerState,
        _env: &Arc<RtEnv>,
        action: &PingerAction,
    ) -> anyhow::Result<(PingerState, Effects<Self>)> {
        let PingerAction::Ping(n) = action;
        if *n < 0 {
            anyhow::bail!("no negative pings");
        }
        Ok((
            state.clone(),
            Effects::none().send::<PongerMachine>("pong1".to_string(), PongerAction::Pong(*n)),
        ))
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
