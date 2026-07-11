use crate::effects::Effects;
use crate::machine::{EntityId, Identified, StateMachine};
use crate::store::{store, CallToken, OutboxRow, RowKind, SaveOutcome, TransitionWrite};
use chrono::Utc;
use dashmap::DashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use tokio::sync::{mpsc, oneshot};

/// Mailbox message. `Act` is fire-and-forget (external callers); `Tracked` carries a durable
/// outbox row's identity and an ack channel — the sender's dispatcher deletes the row only
/// after the receiver commits. `Drain` (sweep wake of a live actor) re-reads the pending
/// outbox and redispatches it; duplicate deliveries this causes are absorbed by monotonic
/// receiver dedup / idempotent constructs, so a spurious drain is safe.
enum Envelope<A> {
    Act(A),
    Drain,
    Tracked {
        action: A,
        token: CallToken,
        ack: oneshot::Sender<DeliveryOutcome>,
    },
}

enum DeliveryOutcome {
    /// Transition committed (dedup row written with it).
    Applied,
    /// Receiver had already applied this exact call — safe to ack.
    Duplicate,
    /// Domain rejection (invalid transition, unconstructed target, unparseable row) —
    /// permanent; the row gets poisoned, never retried.
    Rejected(String),
}

/// Dropped ack (receiver died mid-processing) or infra failure — retry the delivery.
struct TransientDelivery(String);

/// A parsed entity row image: the unit that flows from store to actor and back.
struct Persisted<SM: StateMachine> {
    state: SM::State,
    version: i64,
    next_seq: i64,
}

enum LoadStatus {
    /// Actor is now live (was already, or just spawned from its row).
    Live,
    /// No row in the store.
    Absent,
    /// Row exists but its state doesn't deserialize (logged where detected).
    Corrupt,
}

type DeliverFuture = Pin<Box<dyn Future<Output = Result<DeliveryOutcome, TransientDelivery>> + Send>>;
type Deliverer = Box<dyn Fn(RowKind, String, String, CallToken) -> DeliverFuture + Send + Sync>;
type WakeFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type Waker = Box<dyn Fn(String) -> WakeFuture + Send + Sync>;

/// The runtime registry: every machine type registers once at startup. `HANDLES` gives typed
/// access (`handle::<SM>()`); `DELIVERERS` routes outbox rows — which are strings of data —
/// back to a concrete `StateMachine` impl, including after a restart.
static HANDLES: OnceLock<DashMap<std::any::TypeId, &'static (dyn std::any::Any + Send + Sync)>> =
    OnceLock::new();
static DELIVERERS: OnceLock<DashMap<String, Deliverer>> = OnceLock::new();
static WAKERS: OnceLock<DashMap<String, Waker>> = OnceLock::new();

fn handles() -> &'static DashMap<std::any::TypeId, &'static (dyn std::any::Any + Send + Sync)> {
    HANDLES.get_or_init(DashMap::new)
}

fn deliverers() -> &'static DashMap<String, Deliverer> {
    DELIVERERS.get_or_init(DashMap::new)
}

pub(crate) fn wakers() -> &'static DashMap<String, Waker> {
    WAKERS.get_or_init(DashMap::new)
}

/// Register a machine type with its environment. Call once per machine at startup, after
/// `init_turso_store`. Handles live for the program's lifetime (they were per-machine statics
/// before; now the runtime owns them).
pub fn register<SM: StateMachine>(env: SM::Env) {
    let leaked: &'static StateMachineHandle<SM> = Box::leak(Box::new(StateMachineHandle {
        entities: Arc::new(DashMap::new()),
        env: Arc::new(env),
    }));
    let already = handles()
        .insert(std::any::TypeId::of::<SM>(), leaked as &(dyn std::any::Any + Send + Sync))
        .is_some();
    assert!(!already, "state machine {} registered twice", SM::name());
    wakers().insert(
        SM::name().to_string(),
        Box::new(|id_json| {
            Box::pin(async move {
                match serde_json::from_str::<SM::Id>(&id_json) {
                    Ok(id) => handle::<SM>().wake(&id).await,
                    Err(e) => log_transition::<SM>(&format!("sweep skipped unparseable id {id_json}: {e}")),
                }
            })
        }),
    );
    let name_clash = deliverers().insert(
        SM::name().to_string(),
        Box::new(|kind, id_json, payload_json, token| {
            Box::pin(async move {
                match kind {
                    RowKind::Act => {
                        let Ok(id) = serde_json::from_str::<SM::Id>(&id_json) else {
                            return Ok(DeliveryOutcome::Rejected(format!("unparseable target id: {id_json}")));
                        };
                        let Ok(action) = serde_json::from_str::<SM::Action>(&payload_json) else {
                            return Ok(DeliveryOutcome::Rejected(format!("unparseable action for {}", SM::name())));
                        };
                        handle::<SM>().deliver_tracked(id, action, token).await
                    }
                    RowKind::Construct => {
                        let Ok(construction) = serde_json::from_str::<SM::Construction>(&payload_json) else {
                            return Ok(DeliveryOutcome::Rejected(format!("unparseable construction for {}", SM::name())));
                        };
                        handle::<SM>().deliver_construct(construction).await
                    }
                }
            })
        }),
    ).is_some();
    // two different types with one name would silently steal each other's outbox routing
    assert!(!name_clash, "two state machine types registered with the name {}", SM::name());
}

/// Typed access to a registered machine's handle.
pub fn handle<SM: StateMachine>() -> &'static StateMachineHandle<SM> {
    let entry = handles()
        .get(&std::any::TypeId::of::<SM>())
        .unwrap_or_else(|| {
            panic!(
                "state machine {} not registered — call re_framework::register::<{}>(env) at startup",
                SM::name(),
                SM::name()
            )
        });
    let any: &'static (dyn std::any::Any + Send + Sync) = *entry.value();
    any.downcast_ref::<StateMachineHandle<SM>>()
        .expect("registry entry type matches its TypeId key")
}

/// Sole sender to a live actor's mailbox; not `Clone` — dropping it (via registry removal) stops the actor (RAII).
/// `generation` identifies which activation owns the slot, so a dying actor's cleanup can't
/// evict a successor that already replaced it.
struct SoleMailboxHandle<SM: StateMachine> {
    sender: mpsc::UnboundedSender<Envelope<SM::Action>>,
    generation: u64,
}

fn next_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct StateMachineHandle<SM: StateMachine> {
    entities: Arc<DashMap<String, SoleMailboxHandle<SM>>>,
    env: Arc<SM::Env>,
}

impl<SM: StateMachine> StateMachineHandle<SM> {
    /// Send an envelope if the actor is live; hand it back if not. Never blocks, never awaits —
    /// safe to call while a map shard guard is held (send on an unbounded channel is sync).
    fn try_send(&self, key: &str, envelope: Envelope<SM::Action>) -> Result<(), Envelope<SM::Action>> {
        match self.entities.get(key) {
            Some(handle) => handle.sender.send(envelope).map_err(|e| e.0),
            None => Err(envelope),
        }
    }

    /// Spawn an actor for a parsed row image unless one is already live. All I/O happened
    /// before this; the entry guard is held only for the sync check-and-insert (the narrowed lock).
    /// Durable outbox rows are NOT handed over here — the actor's drain-on-activate reads them
    /// from the store, the single source, so nothing can be dispatched twice at activation.
    fn spawn_if_vacant(&self, id: SM::Id, persisted: Persisted<SM>) {
        use dashmap::mapref::entry::Entry as DEntry;
        match self.entities.entry(id.get_id_string()) {
            DEntry::Occupied(_) => {}
            DEntry::Vacant(slot) => {
                let generation = next_generation();
                let (tx, rx) = mpsc::unbounded_channel();
                tokio::spawn(run_entity::<SM>(
                    persisted,
                    rx,
                    EntityContext::new(&id, generation, Arc::clone(&self.env), Arc::clone(&self.entities)),
                ));
                slot.insert(SoleMailboxHandle { sender: tx, generation });
            }
        }
    }

    /// Sweep wake. A live actor is told to re-drain its outbox — pending rows must not hide
    /// behind a long-lived actor whose activation drain failed or predates them. A dead one
    /// is loaded and spawned; drain-on-activate covers the rest. Carries no authority either
    /// way: the actor re-reads everything from the store.
    async fn wake(&self, id: &SM::Id) {
        let key = id.get_id_string();
        if self.try_send(&key, Envelope::Drain).is_ok() {
            return;
        }
        if let Err(e) = self.ensure_live(id, &key).await {
            log_transition::<SM>(&format!("sweep wake failed for {key}: {e:#}"));
        }
    }

    /// The one load→parse→spawn path. After `Ok(Live)` the actor is guaranteed live
    /// (already was, or just spawned from its row); `Corrupt` is logged here so the
    /// interim reset policy stays single-sourced.
    async fn ensure_live(&self, id: &SM::Id, key: &str) -> anyhow::Result<LoadStatus> {
        if self.entities.contains_key(key) {
            return Ok(LoadStatus::Live);
        }
        match store().load(SM::name(), key).await? {
            None => Ok(LoadStatus::Absent),
            Some(loaded) => match serde_json::from_str::<SM::State>(&loaded.state_json) {
                Ok(state) => {
                    let persisted = Persisted {
                        state,
                        version: loaded.version,
                        next_seq: loaded.next_outbox_seq,
                    };
                    self.spawn_if_vacant(id.clone(), persisted);
                    Ok(LoadStatus::Live)
                }
                Err(e) => {
                    log_transition::<SM>(&format!("stored state failed to deserialize: {e}"));
                    Ok(LoadStatus::Corrupt)
                }
            },
        }
    }

    pub async fn maybe_construct(&self, construction: SM::Construction) {
        let id = construction.get_id().clone();
        let key = id.get_id_string();
        match self.ensure_live(&id, &key).await {
            Err(e) => log_transition::<SM>(&format!("construct aborted — load failed: {e:#}")),
            Ok(LoadStatus::Live) => {}
            Ok(LoadStatus::Absent) => {
                let _ = self.construct_fresh(id, key, construction).await;
            }
            Ok(LoadStatus::Corrupt) => {
                // interim policy (#186): corrupt state resets to a fresh construct;
                // proper corruption handling arrives with state versioning
                log_transition::<SM>("resetting entity — stored state unparseable");
                match store().delete(SM::name(), &key).await {
                    Ok(()) => {
                        let _ = self.construct_fresh(id, key, construction).await;
                    }
                    Err(e) => log_transition::<SM>(&format!("reset failed — construct aborted: {e:#}")),
                }
            }
        }
    }

    /// Construct-if-absent core. `Ok` covers both "constructed" and benign races (someone else
    /// constructed / a delete raced); `Err` is a store failure the caller may retry.
    async fn construct_fresh(&self, id: SM::Id, key: String, construction: SM::Construction) -> anyhow::Result<()> {
        let mut effects = Effects::new(id.clone());
        let state = SM::construct(construction, &mut effects);
        let state_json = serde_json::to_string(&state).map_err(|e| {
            log_transition::<SM>("construct aborted — state failed to serialize");
            anyhow::anyhow!("state failed to serialize: {e}")
        })?;
        let id_json = serde_json::to_string(&id).expect("EntityId serializes");
        match store()
            .insert(SM::name(), &key, &id_json, &state_json, tick_deadline::<SM>(&state), &effects.actions)
            .await
        {
            Err(e) => {
                log_transition::<SM>(&format!("construct aborted — persistence failed: {e:#}"));
                Err(e)
            }
            Ok(SaveOutcome::Conflict { .. }) => {
                match self.ensure_live(&id, &key).await {
                    Ok(LoadStatus::Live) => {}
                    Ok(LoadStatus::Absent) => log_transition::<SM>("construct raced a delete; dropping construction"),
                    Ok(LoadStatus::Corrupt) => {
                        log_transition::<SM>("construct raced another corrupt row; dropping construction")
                    }
                    Err(e) => log_transition::<SM>(&format!("construct raced; reload failed: {e:#}")),
                }
                Ok(())
            }
            Ok(SaveOutcome::Ok) => {
                // the committed construct rows reach the dispatcher via drain-on-activate
                // ONLY — handing them over here as well would dispatch every one twice
                let persisted = Persisted {
                    state,
                    version: 0,
                    next_seq: effects.actions.len() as i64,
                };
                self.spawn_if_vacant(id, persisted);
                for external in effects.externals {
                    tokio::spawn(external);
                }
                Ok(())
            }
        }
    }

    /// A durable Construct row arriving from another entity's outbox. Idempotent by nature —
    /// an existing target is a successful no-op, so constructs need no dedup slot.
    async fn deliver_construct(&self, construction: SM::Construction) -> Result<DeliveryOutcome, TransientDelivery> {
        let id = construction.get_id().clone();
        let key = id.get_id_string();
        match self.ensure_live(&id, &key).await {
            Err(e) => Err(TransientDelivery(format!("load failed: {e:#}"))),
            Ok(LoadStatus::Live) => Ok(DeliveryOutcome::Duplicate),
            Ok(LoadStatus::Corrupt) => {
                log_transition::<SM>("resetting entity — stored state unparseable");
                match store().delete(SM::name(), &key).await {
                    Err(e) => Err(TransientDelivery(format!("reset failed: {e:#}"))),
                    Ok(()) => self
                        .construct_fresh(id, key, construction)
                        .await
                        .map(|()| DeliveryOutcome::Applied)
                        .map_err(|e| TransientDelivery(format!("{e:#}"))),
                }
            }
            Ok(LoadStatus::Absent) => self
                .construct_fresh(id, key, construction)
                .await
                .map(|()| DeliveryOutcome::Applied)
                .map_err(|e| TransientDelivery(format!("{e:#}"))),
        }
    }

    pub async fn act(&self, id: SM::Id, action: SM::Action) {
        let key = id.get_id_string();
        let mut envelope = Envelope::Act(action);
        for _ in 0..2 {
            envelope = match self.try_send(&key, envelope) {
                Ok(()) => return,
                Err(back) => back,
            };
            match self.ensure_live(&id, &key).await {
                Err(e) => {
                    log_transition::<SM>(&format!("act dropped — load failed: {e:#}"));
                    return;
                }
                Ok(LoadStatus::Live) => {}
                Ok(LoadStatus::Absent) => {
                    let Envelope::Act(action) = &envelope else { unreachable!("act sends Act") };
                    println!(
                        "[warn] action {action:?} for unconstructed entity {key}; dropping (maybe_construct must precede act)"
                    );
                    return;
                }
                Ok(LoadStatus::Corrupt) => {
                    log_transition::<SM>("act dropped — stored state unparseable (resets on next maybe_construct)");
                    return;
                }
            }
        }
        log_transition::<SM>("act dropped — actor kept vanishing (raced deletes?)");
    }

    /// Construct-if-absent, then act: the everyday frontend entry point (subject's
    /// ActMaybeConstruct). `act` alone still exists for actions to entities that must
    /// already exist (e.g. externals feeding results back).
    pub async fn act_maybe_construct(&self, construction: SM::Construction, action: SM::Action) {
        let id = construction.get_id().clone();
        self.maybe_construct(construction).await;
        self.act(id, action).await;
    }

    /// Deliver a durable outbox row's action and wait for the receiver's verdict.
    async fn deliver_tracked(
        &self,
        id: SM::Id,
        action: SM::Action,
        token: CallToken,
    ) -> Result<DeliveryOutcome, TransientDelivery> {
        let key = id.get_id_string();
        let (ack_tx, ack_rx) = oneshot::channel();
        let mut envelope = Envelope::Tracked { action, token, ack: ack_tx };
        let mut sent = false;
        for _ in 0..2 {
            envelope = match self.try_send(&key, envelope) {
                Ok(()) => {
                    sent = true;
                    break;
                }
                Err(back) => back,
            };
            match self.ensure_live(&id, &key).await {
                Err(e) => return Err(TransientDelivery(format!("load failed: {e:#}"))),
                Ok(LoadStatus::Live) => {}
                Ok(LoadStatus::Absent) => {
                    return Ok(DeliveryOutcome::Rejected(format!("unconstructed entity {key}")))
                }
                Ok(LoadStatus::Corrupt) => {
                    return Ok(DeliveryOutcome::Rejected(format!(
                        "entity {key} state unparseable (resets on next maybe_construct)"
                    )))
                }
            }
        }
        if !sent {
            return Err(TransientDelivery("actor kept vanishing".to_string()));
        }
        ack_rx
            .await
            .map_err(|_| TransientDelivery("receiver dropped without ack".to_string()))
    }

    pub async fn delete(&self, id: SM::Id) {
        self.entities.remove(&id.get_id_string());
        if let Err(e) = store().delete(SM::name(), &id.get_id_string()).await {
            log_transition::<SM>(&format!("delete — {e:#}"));
        }
    }
}

struct EntityContext<SM: StateMachine> {
    id: SM::Id,
    id_string: String,
    id_json: String,
    generation: u64,
    env: Arc<SM::Env>,
    entities: Arc<DashMap<String, SoleMailboxHandle<SM>>>,
}

impl<SM: StateMachine> EntityContext<SM> {
    fn new(
        id: &SM::Id,
        generation: u64,
        env: Arc<SM::Env>,
        entities: Arc<DashMap<String, SoleMailboxHandle<SM>>>,
    ) -> Self {
        EntityContext {
            id: id.clone(),
            id_string: id.get_id_string(),
            id_json: serde_json::to_string(id).expect("EntityId serializes"),
            generation,
            env,
            entities,
        }
    }
}

/// Re-read the pending outbox from the store and hand it to the dispatcher. Used at
/// activation and on `Drain` wakes. May re-send rows the dispatcher already has in flight —
/// harmless: redelivery is absorbed by monotonic receiver dedup / idempotent constructs,
/// and acking an already-deleted row is a no-op.
async fn drain_outbox<SM: StateMachine>(
    id_string: &str,
    dispatch_tx: &mpsc::UnboundedSender<Vec<OutboxRow>>,
) {
    match store().pending_outbox(SM::name(), id_string).await {
        Ok(pending) if !pending.is_empty() => {
            let _ = dispatch_tx.send(pending);
        }
        Ok(_) => {}
        Err(e) => log_transition::<SM>(&format!("outbox drain failed (sweep will re-wake): {e:#}")),
    }
}

async fn run_entity<SM: StateMachine>(
    persisted: Persisted<SM>,
    mut rx: mpsc::UnboundedReceiver<Envelope<SM::Action>>,
    ctx: EntityContext<SM>,
) {
    let Persisted {
        mut state,
        mut version,
        mut next_seq,
    } = persisted;
    let (dispatch_tx, dispatch_rx) = mpsc::unbounded_channel::<Vec<OutboxRow>>();
    // dropped when this actor ends, telling the dispatcher to stop: an orphaned dispatcher
    // must not keep retrying (or acking) rows a successor activation now owns
    let (_dispatcher_stop, stopped) = tokio::sync::watch::channel(());
    tokio::spawn(run_dispatcher(SM::name(), ctx.id_string.clone(), dispatch_rx, stopped));

    // drain-on-activate: redispatch every un-acked durable row before serving new work
    // (construct-time rows arrive this way too — the store is the only handover)
    drain_outbox::<SM>(&ctx.id_string, &dispatch_tx).await;

    loop {
        let envelope = match SM::schedule(&state) {
            None => rx.recv().await,
            Some(scheduled) => {
                let delay = (scheduled.at - Utc::now())
                    .to_std()
                    .unwrap_or(std::time::Duration::ZERO);

                // Firing the pre-computed action is safe only because this loop is the sole
                // writer of `state` — nothing can change it between deriving and firing. Any
                // future out-of-loop tick delivery (e.g. a sweep) must recompute from state.
                tokio::time::timeout(delay, rx.recv())
                    .await
                    .unwrap_or_else(|_e| Some(Envelope::Act(scheduled.action)))
            }
        };

        let Some(envelope) = envelope else {
            log_transition::<SM>("Delete");
            break;
        };
        let (action, mut tracked) = match envelope {
            Envelope::Drain => {
                drain_outbox::<SM>(&ctx.id_string, &dispatch_tx).await;
                continue;
            }
            Envelope::Act(action) => (action, None),
            Envelope::Tracked { action, token, ack } => (action, Some((token, ack))),
        };

        if let Some((token, ack)) = tracked.take() {
            match store().is_duplicate(SM::name(), &ctx.id_string, &token).await {
                Ok(true) => {
                    let _ = ack.send(DeliveryOutcome::Duplicate);
                    continue;
                }
                Ok(false) => tracked = Some((token, ack)),
                Err(e) => {
                    // no ack — the sender's dispatcher retries the delivery
                    log_transition::<SM>(&format!("dedup check failed, deferring delivery: {e:#}"));
                    continue;
                }
            }
        }

        log_transition::<SM>(&format!("Action: {action:?}"));
        let mut effects = Effects::new(ctx.id.clone());
        match SM::transition(&state, &ctx.id, &ctx.env, &action, &mut effects) {
            Ok(next) => {
                let Ok(state_json) = serde_json::to_string(&next) else {
                    log_transition::<SM>("aborted — state failed to serialize");
                    continue;
                };
                let write = TransitionWrite {
                    machine: SM::name(),
                    id_string: ctx.id_string.clone(),
                    state_json,
                    expected_version: version,
                    first_seq: next_seq,
                    next_outbox_seq: next_seq + effects.actions.len() as i64,
                    next_tick_on: tick_deadline::<SM>(&next),
                    outbox: effects.actions,
                    dedup: tracked.as_ref().map(|(token, _)| token.clone()),
                };
                match store().save(&write, &ctx.id_json).await {
                    Ok(SaveOutcome::Ok) => {
                        state = next;
                        version += 1;
                        next_seq = write.next_outbox_seq;
                        if !write.outbox.is_empty() {
                            let _ = dispatch_tx.send(rows_from_drafts(write.first_seq, &write.outbox));
                        }
                        for external in effects.externals {
                            tokio::spawn(external);
                        }
                        if let Some((_, ack)) = tracked {
                            let _ = ack.send(DeliveryOutcome::Applied);
                        }
                    }
                    Ok(SaveOutcome::Conflict { actual }) => {
                        // reload-and-drop (#186): a CAS miss means this instance is illegitimate —
                        // kill it; the next message rebuilds from the store. Dropping the ack makes
                        // the sender's dispatcher retry against the rebuilt actor.
                        log_transition::<SM>(&format!(
                            "CAS CONFLICT (expected v{version}, store has {actual:?}) — dropping actor; state rebuilds from store"
                        ));
                        // generation-checked: a concurrent delete + reconstruct may already
                        // have installed a successor, which must not be evicted by our death
                        ctx.entities
                            .remove_if(&ctx.id_string, |_, current| current.generation == ctx.generation);
                        break;
                    }
                    Err(e) => {
                        // no ack on I/O failure — sender retries; state unchanged
                        log_transition::<SM>(&format!("aborted — persistence failed: {e:#}"));
                    }
                }
            }
            Err(err) => {
                log_transition::<SM>(&format!("dropped — no state change: {err}"));
                if let Some((_, ack)) = tracked {
                    let _ = ack.send(DeliveryOutcome::Rejected(err.to_string()));
                }
            }
        }
    }
}

/// The persisted mirror of `schedule()`: written into the entity row at every commit so the
/// sweep can find due timers without deserializing state. The wake carries no authority — the
/// woken actor recomputes `schedule()` from state and fires (or doesn't) from that.
fn tick_deadline<SM: StateMachine>(state: &SM::State) -> Option<i64> {
    SM::schedule(state).map(|scheduled| scheduled.at.timestamp_millis())
}

fn rows_from_drafts(first_seq: i64, drafts: &[crate::store::OutboxDraft]) -> Vec<OutboxRow> {
    drafts
        .iter()
        .enumerate()
        .map(|(offset, draft)| OutboxRow {
            seq: first_seq + offset as i64,
            kind: draft.kind,
            target_machine: draft.target_machine.to_string(),
            target_id_json: draft.target_id_json.clone(),
            payload_json: draft.payload_json.clone(),
        })
        .collect()
}

/// Per-entity outbound dispatcher: strictly serial per sender, delete-on-ack,
/// poison-on-rejection, retry-forever on transient. Stops when `stopped` closes (its actor
/// ended): whatever it still holds stays in the outbox for the successor activation, and it
/// must not race that successor with deliveries or acks of rows it no longer owns.
async fn run_dispatcher(
    machine: &'static str,
    sender_id: String,
    mut rx: mpsc::UnboundedReceiver<Vec<OutboxRow>>,
    mut stopped: tokio::sync::watch::Receiver<()>,
) {
    while let Some(batch) = rx.recv().await {
        for row in batch {
            if !dispatch_row(machine, &sender_id, row, &mut stopped).await {
                return;
            }
        }
    }
}

/// Resolve one row to a definitive verdict. Returns false if the actor ended first —
/// the row remains in the outbox for the successor.
async fn dispatch_row(
    machine: &'static str,
    sender_id: &str,
    row: OutboxRow,
    stopped: &mut tokio::sync::watch::Receiver<()>,
) -> bool {
    let mut attempt = 0u32;
    loop {
        if stopped.has_changed().is_err() {
            return false;
        }
        let token = CallToken {
            sender_machine: machine,
            sender_id: sender_id.to_string(),
            seq: row.seq,
        };
        let outcome = match deliverers().get(&row.target_machine) {
            // transient, not a rejection: registration may simply not have happened yet
            // (startup ordering) — poisoning here would destroy the delivery irreversibly
            None => Err(TransientDelivery(format!(
                "no machine named {} registered yet",
                row.target_machine
            ))),
            Some(deliver) => deliver(row.kind, row.target_id_json.clone(), row.payload_json.clone(), token).await,
        };
        match outcome {
            // ack-vs-execute separation: after a definitive verdict, only the storage ack is
            // retried — the delivery is never re-run in this session
            Ok(DeliveryOutcome::Applied | DeliveryOutcome::Duplicate) => {
                ack_with_retry(machine, sender_id, row.seq, stopped).await;
                return true;
            }
            Ok(DeliveryOutcome::Rejected(reason)) => {
                println!("[outbox] {machine}/{sender_id} seq {} rejected by {}: {reason}", row.seq, row.target_machine);
                if let Err(e) = store().fail_outbox(machine, sender_id, row.seq, &reason).await {
                    println!("[outbox] failed to poison row seq {}: {e:#}", row.seq);
                }
                return true;
            }
            Err(TransientDelivery(reason)) => {
                attempt += 1;
                let delay = std::time::Duration::from_secs(2u64.pow(attempt.min(6)).min(60));
                println!(
                    "[outbox] {machine}/{sender_id} seq {} transient delivery failure (attempt {attempt}, retrying in {delay:?}): {reason}",
                    row.seq
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = stopped.changed() => return false,
                }
            }
        }
    }
}

async fn ack_with_retry(
    machine: &'static str,
    sender_id: &str,
    seq: i64,
    stopped: &mut tokio::sync::watch::Receiver<()>,
) {
    for attempt in 0..5 {
        match store().ack_outbox(machine, sender_id, seq).await {
            Ok(()) => return,
            Err(e) => {
                println!("[outbox] ack failed for {machine}/{sender_id} seq {seq} (attempt {attempt}): {e:#}");
                if stopped.has_changed().is_err() {
                    // actor ended: stop touching this lane — a successor now owns the row,
                    // and its redelivery resolves to Duplicate and acks
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
    // give up: the row survives and redelivers on a later activation, where it resolves to
    // Duplicate (monotonic dedup — the verdict here was definitive) and gets acked then
}

fn log_transition<SM: StateMachine>(label: &str) {
    println!(
        "TRANSITION AT {} - StateMachine: {} - {}",
        Utc::now(),
        SM::name(),
        label
    );
}
