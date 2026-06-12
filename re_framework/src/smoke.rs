
use crate::{Effects, Scheduled, StateMachine, StateMachineHandle};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration as StdDuration;


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

#[derive(Serialize, Deserialize)]
enum CounterAction {
    Add(i64),
    Ping, 
    Tick,
}

struct CounterInit {
    start: i64,
}

impl StateMachine for CounterMachine {
    type State = CounterState;
    type Id = String;
    type Action = CounterAction;
    type Construction = CounterInit;
    type Env = CounterEnv;

    fn construct(_id: String, init: CounterInit) -> CounterState {
        CounterState {
            total: init.start,
            tick_at: Some(Utc::now() + Duration::milliseconds(50)),
        }
    }

    fn transition(
        mut state: CounterState,
        _id: &String,
        env: Arc<CounterEnv>,
        action: &CounterAction,
    ) -> anyhow::Result<(CounterState, Effects<Self>)> {
        match action {
            CounterAction::Add(n) => {
                if state.total + n < 0 {
                    anyhow::bail!("would go negative"); 
                }
                state.total += n;
                env.obs.lock().unwrap().totals.push(state.total);
                Ok((state, Effects::none()))
            }
            CounterAction::Ping => Ok((state, Effects::none().then(async { CounterAction::Add(10) }))),
            CounterAction::Tick => {
                state.tick_at = None;
                env.obs.lock().unwrap().ticks += 1;
                Ok((state, Effects::none()))
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
    let sm = StateMachineHandle::<CounterMachine>::new(CounterEnv { obs: obs.clone() });
    COUNTER.set(sm.clone()).ok().expect("set COUNTER once");

    sm.maybe_construct("c1".to_string(), CounterInit { start: 0 }).await;
    sm.act("c1".to_string(), CounterAction::Add(5)).await; 

    sm.maybe_construct("c1".to_string(), CounterInit { start: 999 }).await;
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
#[derive(Serialize, Deserialize)]
enum PongerAction {
    Pong(i64),
}
struct PongerInit;

impl StateMachine for PongerMachine {
    type State = PongerState;
    type Id = String;
    type Action = PongerAction;
    type Construction = PongerInit;
    type Env = RtEnv;
    fn construct(_id: String, _: PongerInit) -> PongerState {
        PongerState
    }
    fn transition(
        state: PongerState,
        _id: &String,
        env: Arc<RtEnv>,
        action: &PongerAction,
    ) -> anyhow::Result<(PongerState, Effects<Self>)> {
        let PongerAction::Pong(n) = action;
        env.received.lock().unwrap().push(*n);
        Ok((state, Effects::none()))
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
#[derive(Serialize, Deserialize)]
enum PingerAction {
    Ping(i64),
}
struct PingerInit;

impl StateMachine for PingerMachine {
    type State = PingerState;
    type Id = String;
    type Action = PingerAction;
    type Construction = PingerInit;
    type Env = RtEnv;
    fn construct(_id: String, _: PingerInit) -> PingerState {
        PingerState
    }
    fn transition(
        state: PingerState,
        _id: &String,
        _env: Arc<RtEnv>,
        action: &PingerAction,
    ) -> anyhow::Result<(PingerState, Effects<Self>)> {
        let PingerAction::Ping(n) = action;
        if *n < 0 {
            anyhow::bail!("no negative pings"); 
        }
        Ok((
            state,
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
    let ponger = StateMachineHandle::<PongerMachine>::new(RtEnv { received: received.clone() });
    PONGER.set(ponger.clone()).ok().expect("set PONGER once");
    let pinger = StateMachineHandle::<PingerMachine>::new(RtEnv { received: received.clone() });
    PINGER.set(pinger.clone()).ok().expect("set PINGER once");

    ponger.maybe_construct("pong1".to_string(), PongerInit).await;
    pinger.maybe_construct("ping1".to_string(), PingerInit).await;

    pinger.act("ping1".to_string(), PingerAction::Ping(42)).await; 
    pinger.act("ping1".to_string(), PingerAction::Ping(-1)).await; 

    tokio::time::sleep(StdDuration::from_millis(50)).await;
    assert_eq!(
        *received.lock().unwrap(),
        vec![42],
        "outbound fired only for the committed transition, and reached the other machine"
    );
}
