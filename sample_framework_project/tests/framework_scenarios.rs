//! Failure scenarios from the adversarial review of #186 / PR #188, encoded as regression
//! tests. Each test reproduces one flaw by driving the framework exactly the way a crash,
//! race, or misconfiguration would — injecting crash-leftover outbox rows and CAS bumps
//! straight into the SQLite file, the same way the manual drills in the README do.
//!
//! The registries and store are process-global, so the tests share one temp database and
//! run serialized (see `SERIAL`); every scenario uses its own entity ids.

use re_framework::{handle, register, Effects, EntityId, Identified, Scheduled, StateMachine};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------------------------------
// shared plumbing

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn db_path() -> PathBuf {
    std::env::temp_dir().join(format!("re_fw_scenarios_{}.db", std::process::id()))
}

async fn setup() {
    static INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    INIT.get_or_init(|| async {
        let path = db_path();
        let _ = std::fs::remove_file(&path);
        re_framework::init_turso_store(path.to_str().expect("utf8 temp path"))
            .await
            .expect("init scenario store");
        register::<RecvMachine>(());
        register::<SenderMachine>(());
        register::<CtorSpamMachine>(());
    })
    .await;
}

/// Second handle onto the same SQLite file, for injecting crash leftovers / CAS bumps.
async fn raw_conn() -> turso::Connection {
    let db = turso::Builder::new_local(db_path().to_str().expect("utf8 temp path"))
        .build()
        .await
        .expect("open raw db handle");
    let conn = db.connect().expect("raw connect");
    conn.busy_timeout(Duration::from_secs(5)).expect("raw busy_timeout");
    conn
}

fn recorded() -> &'static Mutex<HashMap<String, Vec<i64>>> {
    static RECORDED: OnceLock<Mutex<HashMap<String, Vec<i64>>>> = OnceLock::new();
    RECORDED.get_or_init(Default::default)
}

fn values(receiver: &str) -> Vec<i64> {
    recorded().lock().expect("recorded lock").get(receiver).cloned().unwrap_or_default()
}

async fn entity_version(machine: &str, id: &str) -> Option<i64> {
    let conn = raw_conn().await;
    let mut rows = conn
        .query(
            "SELECT version FROM entities WHERE machine = ? AND id = ?",
            (machine, id),
        )
        .await
        .expect("query entity version");
    rows.next()
        .await
        .expect("version row")
        .map(|row| row.get::<i64>(0).expect("version value"))
}

async fn pending_outbox_count(machine: &str, sender_id: &str) -> i64 {
    let conn = raw_conn().await;
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM outbox WHERE sender_machine = ? AND sender_id = ? AND failure IS NULL",
            (machine, sender_id),
        )
        .await
        .expect("count pending outbox");
    rows.next()
        .await
        .expect("count row")
        .expect("count present")
        .get::<i64>(0)
        .expect("count value")
}

async fn wait_for(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + timeout;
    while !cond() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_outbox_empty(machine: &str, sender_id: &str, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    while pending_outbox_count(machine, sender_id).await > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {machine}/{sender_id} outbox to drain"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------------------
// scenario machines

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Sid(String);

impl EntityId for Sid {
    fn get_id_string(&self) -> String {
        self.0.clone()
    }
}

/// Records every value it receives into `recorded()`, keyed by its own id — the observation
/// point every scenario asserts against.
struct RecvMachine;
#[derive(Clone, Serialize, Deserialize)]
struct RecvState;
#[derive(Debug, Serialize, Deserialize)]
enum RecvAction {
    Val(i64),
}
#[derive(Serialize, Deserialize)]
struct RecvInit {
    id: Sid,
}
impl Identified for RecvInit {
    type Id = Sid;
    fn get_id(&self) -> &Sid {
        &self.id
    }
}
impl StateMachine for RecvMachine {
    type State = RecvState;
    type Id = Sid;
    type Action = RecvAction;
    type Construction = RecvInit;
    type Env = ();
    fn construct(_init: RecvInit, _effects: &mut Effects<Self>) -> RecvState {
        RecvState
    }
    fn transition(
        state: &RecvState,
        id: &Sid,
        _env: &Arc<()>,
        action: &RecvAction,
        _effects: &mut Effects<Self>,
    ) -> anyhow::Result<RecvState> {
        let RecvAction::Val(n) = action;
        recorded().lock().expect("recorded lock").entry(id.0.clone()).or_default().push(*n);
        Ok(state.clone())
    }
    fn schedule(_state: &RecvState) -> Option<Scheduled<RecvAction>> {
        None
    }
    fn name() -> &'static str {
        "ScnRecv"
    }
}

/// Sends durable actions on command; `Noop` commits without effects (used to force
/// activations and CAS conflicts).
struct SenderMachine;
#[derive(Clone, Serialize, Deserialize)]
struct SenderState;
#[derive(Debug, Serialize, Deserialize)]
enum SenderAction {
    Send { to: String, v: i64 },
    SendToLate { to: String, v: i64 },
    Noop,
}
#[derive(Serialize, Deserialize)]
struct SenderInit {
    id: Sid,
}
impl Identified for SenderInit {
    type Id = Sid;
    fn get_id(&self) -> &Sid {
        &self.id
    }
}
impl StateMachine for SenderMachine {
    type State = SenderState;
    type Id = Sid;
    type Action = SenderAction;
    type Construction = SenderInit;
    type Env = ();
    fn construct(_init: SenderInit, _effects: &mut Effects<Self>) -> SenderState {
        SenderState
    }
    fn transition(
        state: &SenderState,
        _id: &Sid,
        _env: &Arc<()>,
        action: &SenderAction,
        effects: &mut Effects<Self>,
    ) -> anyhow::Result<SenderState> {
        match action {
            SenderAction::Send { to, v } => {
                effects.enqueue_action::<RecvMachine>(Sid(to.clone()), RecvAction::Val(*v));
            }
            SenderAction::SendToLate { to, v } => {
                effects.enqueue_act_maybe_construct::<LateMachine>(
                    LateInit { id: Sid(to.clone()) },
                    LateAction::Val(*v),
                );
            }
            SenderAction::Noop => {}
        }
        Ok(state.clone())
    }
    fn schedule(_state: &SenderState) -> Option<Scheduled<SenderAction>> {
        None
    }
    fn name() -> &'static str {
        "ScnSender"
    }
}

/// Its construct enqueues TWO durable actions to one receiver — the review's C1 probe shape.
struct CtorSpamMachine;
#[derive(Clone, Serialize, Deserialize)]
struct CtorSpamState;
#[derive(Debug, Serialize, Deserialize)]
enum CtorSpamAction {}
#[derive(Serialize, Deserialize)]
struct CtorSpamInit {
    id: Sid,
    target: String,
}
impl Identified for CtorSpamInit {
    type Id = Sid;
    fn get_id(&self) -> &Sid {
        &self.id
    }
}
impl StateMachine for CtorSpamMachine {
    type State = CtorSpamState;
    type Id = Sid;
    type Action = CtorSpamAction;
    type Construction = CtorSpamInit;
    type Env = ();
    fn construct(init: CtorSpamInit, effects: &mut Effects<Self>) -> CtorSpamState {
        effects.enqueue_action::<RecvMachine>(Sid(init.target.clone()), RecvAction::Val(1));
        effects.enqueue_action::<RecvMachine>(Sid(init.target), RecvAction::Val(2));
        CtorSpamState
    }
    fn transition(
        state: &CtorSpamState,
        _id: &Sid,
        _env: &Arc<()>,
        _action: &CtorSpamAction,
        _effects: &mut Effects<Self>,
    ) -> anyhow::Result<CtorSpamState> {
        Ok(state.clone())
    }
    fn schedule(_state: &CtorSpamState) -> Option<Scheduled<CtorSpamAction>> {
        None
    }
    fn name() -> &'static str {
        "ScnCtorSpam"
    }
}

/// Like `RecvMachine`, but deliberately NOT registered at setup — the C5 scenario registers
/// it only after rows targeting it have already been dispatched.
struct LateMachine;
#[derive(Clone, Serialize, Deserialize)]
struct LateState;
#[derive(Debug, Serialize, Deserialize)]
enum LateAction {
    Val(i64),
}
#[derive(Serialize, Deserialize)]
struct LateInit {
    id: Sid,
}
impl Identified for LateInit {
    type Id = Sid;
    fn get_id(&self) -> &Sid {
        &self.id
    }
}
impl StateMachine for LateMachine {
    type State = LateState;
    type Id = Sid;
    type Action = LateAction;
    type Construction = LateInit;
    type Env = ();
    fn construct(_init: LateInit, _effects: &mut Effects<Self>) -> LateState {
        LateState
    }
    fn transition(
        state: &LateState,
        id: &Sid,
        _env: &Arc<()>,
        action: &LateAction,
        _effects: &mut Effects<Self>,
    ) -> anyhow::Result<LateState> {
        let LateAction::Val(n) = action;
        recorded().lock().expect("recorded lock").entry(id.0.clone()).or_default().push(*n);
        Ok(state.clone())
    }
    fn schedule(_state: &LateState) -> Option<Scheduled<LateAction>> {
        None
    }
    fn name() -> &'static str {
        "ScnLate"
    }
}

// ---------------------------------------------------------------------------------------
// C1 — construct-time outbox rows must dispatch exactly once.
// Flaw: construct_fresh handed the freshly committed rows to the actor AND drain-on-activate
// re-read the same rows from the store, so every construct that enqueued 2+ actions to one
// target double-applied all of them (last-call dedup can't catch interleaved seqs).

#[tokio::test]
async fn c1_construct_actions_apply_exactly_once() {
    let _guard = SERIAL.lock().await;
    setup().await;

    handle::<RecvMachine>().maybe_construct(RecvInit { id: Sid("c1_r".into()) }).await;
    handle::<CtorSpamMachine>()
        .maybe_construct(CtorSpamInit { id: Sid("c1_s".into()), target: "c1_r".into() })
        .await;

    wait_for_outbox_empty("ScnCtorSpam", "c1_s", Duration::from_secs(10)).await;
    // grace period: a duplicate dispatch would land shortly after the first batch
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(values("c1_r"), vec![1, 2], "each construct-enqueued action must apply exactly once");
}

// ---------------------------------------------------------------------------------------
// C2/C3 — redelivery of an already-applied row must be absorbed even when the receiver's
// dedup slot has moved past it. Flaw: dedup compared last_seq with `==`, so a crash-leftover
// row with an OLDER seq than the slot was "not a duplicate" and re-applied. This is exactly
// what an ack that gave up (C2) or an orphaned dispatcher racing its successor (C3) leaves
// behind; here the leftover is injected directly, then the actor is killed via a CAS bump
// and respawned so drain-on-activate redelivers it.

#[tokio::test]
async fn c2_c3_stale_seq_redelivery_is_deduped() {
    let _guard = SERIAL.lock().await;
    setup().await;

    let sender = handle::<SenderMachine>();
    handle::<RecvMachine>().maybe_construct(RecvInit { id: Sid("c2_r".into()) }).await;
    sender.maybe_construct(SenderInit { id: Sid("c2_s".into()) }).await;

    sender.act(Sid("c2_s".into()), SenderAction::Send { to: "c2_r".into(), v: 1 }).await;
    sender.act(Sid("c2_s".into()), SenderAction::Send { to: "c2_r".into(), v: 2 }).await;
    wait_for("both sends applied", Duration::from_secs(10), || values("c2_r") == vec![1, 2]).await;
    wait_for_outbox_empty("ScnSender", "c2_s", Duration::from_secs(10)).await;

    // crash leftover: seq 0 survives in the outbox although the receiver applied it long ago
    let conn = raw_conn().await;
    conn.execute(
        "INSERT INTO outbox (sender_machine, sender_id, seq, sender_id_json, target_machine, target_id_json, action, kind, created_at)
         VALUES ('ScnSender', 'c2_s', 0, '\"c2_s\"', 'ScnRecv', '\"c2_r\"', ?, 'act', ?)",
        (
            serde_json::to_string(&RecvAction::Val(1)).expect("serialize action"),
            chrono::Utc::now().timestamp_millis(),
        ),
    )
    .await
    .expect("inject leftover outbox row");
    // kill the live actor: bump its version so the next commit CAS-conflicts and it drops
    conn.execute(
        "UPDATE entities SET version = version + 1 WHERE machine = 'ScnSender' AND id = 'c2_s'",
        (),
    )
    .await
    .expect("bump version");

    sender.act(Sid("c2_s".into()), SenderAction::Noop).await; // CAS conflict → actor drops
    tokio::time::sleep(Duration::from_millis(500)).await;
    sender.act(Sid("c2_s".into()), SenderAction::Noop).await; // respawn → drain redelivers seq 0

    wait_for_outbox_empty("ScnSender", "c2_s", Duration::from_secs(10)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(
        values("c2_r"),
        vec![1, 2],
        "redelivered stale seq must be deduped, not re-applied"
    );
}

// ---------------------------------------------------------------------------------------
// C4 — pending rows must not hide behind a live actor. Flaw: a sweep wake was ensure_live,
// a no-op for an already-live actor, so a row that missed its dispatch (drain failure, ack
// give-up) was stuck until the actor died — days, for a chatbot conversation.

#[tokio::test]
async fn c4_sweep_wake_drains_live_actor() {
    let _guard = SERIAL.lock().await;
    setup().await;

    handle::<RecvMachine>().maybe_construct(RecvInit { id: Sid("c4_r".into()) }).await;
    handle::<SenderMachine>().maybe_construct(SenderInit { id: Sid("c4_s".into()) }).await;
    // pin the actor past its activation drain: the drain provably completes before the
    // actor loop processes any envelope, so a committed Noop proves the drain saw an
    // empty outbox — the injected row below can only be found by a later re-drain
    handle::<SenderMachine>().act(Sid("c4_s".into()), SenderAction::Noop).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while entity_version("ScnSender", "c4_s").await != Some(1) {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for noop commit");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // c4_s is LIVE with an empty, fully drained outbox; inject a row aged past grace
    let conn = raw_conn().await;
    conn.execute(
        "INSERT INTO outbox (sender_machine, sender_id, seq, sender_id_json, target_machine, target_id_json, action, kind, created_at)
         VALUES ('ScnSender', 'c4_s', 0, '\"c4_s\"', 'ScnRecv', '\"c4_r\"', ?, 'act', ?)",
        (
            serde_json::to_string(&RecvAction::Val(7)).expect("serialize action"),
            chrono::Utc::now().timestamp_millis() - 600_000,
        ),
    )
    .await
    .expect("inject stalled outbox row");

    re_framework::start_sweeper();

    wait_for("sweep to force-drain the live actor", Duration::from_secs(15), || {
        values("c4_r") == vec![7]
    })
    .await;
    wait_for_outbox_empty("ScnSender", "c4_s", Duration::from_secs(10)).await;
}

// ---------------------------------------------------------------------------------------
// C5 — dispatching to a not-yet-registered machine must be transient, not poison. Flaw: a
// missing deliverer produced Rejected → the row was permanently failed, so any activation
// racing startup registration destroyed deliveries irreversibly.

#[tokio::test]
async fn c5_unregistered_target_is_transient_not_poison() {
    let _guard = SERIAL.lock().await;
    setup().await;

    let sender = handle::<SenderMachine>();
    sender.maybe_construct(SenderInit { id: Sid("c5_s".into()) }).await;
    // LateMachine is not registered yet: dispatch of both rows (construct + act) must fail
    sender
        .act(Sid("c5_s".into()), SenderAction::SendToLate { to: "c5_l".into(), v: 7 })
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    register::<LateMachine>(());

    wait_for("delivery after late registration", Duration::from_secs(20), || {
        values("c5_l") == vec![7]
    })
    .await;
    wait_for_outbox_empty("ScnSender", "c5_s", Duration::from_secs(10)).await;
}

// ---------------------------------------------------------------------------------------
// C6 — delete + recreate of a sender must not leave its old dedup trail behind. Flaw:
// store delete cleared dedup rows where the entity was the RECEIVER but not where it was
// the CALLER, so a recreated sender restarting at seq 0 collided with the stale last_seq
// and its first sends were silently swallowed as duplicates.

#[tokio::test]
async fn c6_recreated_sender_is_not_falsely_deduped() {
    let _guard = SERIAL.lock().await;
    setup().await;

    let sender = handle::<SenderMachine>();
    handle::<RecvMachine>().maybe_construct(RecvInit { id: Sid("c6_r".into()) }).await;
    sender.maybe_construct(SenderInit { id: Sid("c6_s".into()) }).await;
    sender.act(Sid("c6_s".into()), SenderAction::Send { to: "c6_r".into(), v: 1 }).await;
    wait_for("first send applied", Duration::from_secs(10), || values("c6_r") == vec![1]).await;
    wait_for_outbox_empty("ScnSender", "c6_s", Duration::from_secs(10)).await;

    sender.delete(Sid("c6_s".into())).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // reborn sender restarts its outbox at seq 0 — the same seq its first life used
    sender.maybe_construct(SenderInit { id: Sid("c6_s".into()) }).await;
    sender.act(Sid("c6_s".into()), SenderAction::Send { to: "c6_r".into(), v: 9 }).await;

    wait_for("send from recreated sender applied", Duration::from_secs(10), || {
        values("c6_r") == vec![1, 9]
    })
    .await;
}

// ---------------------------------------------------------------------------------------
// Name collision — two different machine types registering the same `name()` must panic at
// startup instead of silently replacing each other's deliverer and misrouting outbox rows.

struct CollideA;
struct CollideB;

macro_rules! collide_machine {
    ($ty:ident) => {
        impl StateMachine for $ty {
            type State = RecvState;
            type Id = Sid;
            type Action = RecvAction;
            type Construction = RecvInit;
            type Env = ();
            fn construct(_init: RecvInit, _effects: &mut Effects<Self>) -> RecvState {
                RecvState
            }
            fn transition(
                state: &RecvState,
                _id: &Sid,
                _env: &Arc<()>,
                _action: &RecvAction,
                _effects: &mut Effects<Self>,
            ) -> anyhow::Result<RecvState> {
                Ok(state.clone())
            }
            fn schedule(_state: &RecvState) -> Option<Scheduled<RecvAction>> {
                None
            }
            fn name() -> &'static str {
                "ScnCollide"
            }
        }
    };
}
collide_machine!(CollideA);
collide_machine!(CollideB);

#[test]
#[should_panic(expected = "ScnCollide")]
fn name_collision_panics_at_registration() {
    register::<CollideA>(());
    register::<CollideB>(());
}
