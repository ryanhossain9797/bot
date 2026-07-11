use crate::effects::Effects;
use crate::machine::{EntityId, Identified, StateMachine};
use crate::store::{new_generation, store, CallToken, OutboxRow, RowKind, SaveOutcome, TransitionWrite};
use chrono::Utc;
use dashmap::DashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use tokio::sync::{mpsc, oneshot};

enum Envelope<A> {
    Act(A),
    Drain,
    Tracked {
        action: A,
        token: CallToken,
        ack: oneshot::Sender<DeliveryOutcome>,
    },
}

pub(crate) enum DeliveryOutcome {
    Applied,
    Duplicate,
    Rejected(String),
}

pub(crate) struct TransientDelivery(String);

struct Persisted<SM: StateMachine> {
    state: SM::State,
    generation: i64,
    version: i64,
    next_seq: i64,
}

enum LoadStatus {
    Live,
    Absent,
    Corrupt,
}

type DeliverFuture = Pin<Box<dyn Future<Output = Result<DeliveryOutcome, TransientDelivery>> + Send>>;
type Deliverer = Box<dyn Fn(RowKind, String, String, CallToken) -> DeliverFuture + Send + Sync>;
type WakeFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type Waker = Box<dyn Fn(String) -> WakeFuture + Send + Sync>;

pub(crate) struct MachineVtable {
    pub deliver: Deliverer,
    pub wake: Waker,
}

static HANDLES: OnceLock<DashMap<std::any::TypeId, &'static (dyn std::any::Any + Send + Sync)>> =
    OnceLock::new();
static MACHINES: OnceLock<DashMap<String, MachineVtable>> = OnceLock::new();

fn handles() -> &'static DashMap<std::any::TypeId, &'static (dyn std::any::Any + Send + Sync)> {
    HANDLES.get_or_init(DashMap::new)
}

pub(crate) fn machines() -> &'static DashMap<String, MachineVtable> {
    MACHINES.get_or_init(DashMap::new)
}

pub fn register<SM: StateMachine>(env: SM::Env) {
    assert!(
        !handles().contains_key(&std::any::TypeId::of::<SM>()),
        "state machine {} registered twice",
        SM::name()
    );
    assert!(
        !machines().contains_key(SM::name()),
        "two state machine types registered with the name {}",
        SM::name()
    );
    let leaked: &'static StateMachineHandle<SM> = Box::leak(Box::new(StateMachineHandle {
        entities: Arc::new(DashMap::new()),
        env: Arc::new(env),
    }));
    handles().insert(std::any::TypeId::of::<SM>(), leaked as &(dyn std::any::Any + Send + Sync));
    machines().insert(
        SM::name().to_string(),
        MachineVtable {
            wake: Box::new(|id_json| {
                Box::pin(async move {
                    match serde_json::from_str::<SM::Id>(&id_json) {
                        Ok(id) => handle::<SM>().wake(&id).await,
                        Err(e) => log_transition::<SM>(&format!("sweep skipped unparseable id {id_json}: {e}")),
                    }
                })
            }),
            deliver: Box::new(|kind, id_json, payload_json, token| {
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
        },
    );
}

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

struct SoleMailboxHandle<SM: StateMachine> {
    sender: mpsc::UnboundedSender<Envelope<SM::Action>>,
    epoch: u64,
}

fn next_epoch() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct StateMachineHandle<SM: StateMachine> {
    entities: Arc<DashMap<String, SoleMailboxHandle<SM>>>,
    env: Arc<SM::Env>,
}

impl<SM: StateMachine> StateMachineHandle<SM> {
    fn try_send(&self, key: &str, envelope: Envelope<SM::Action>) -> Result<(), Envelope<SM::Action>> {
        match self.entities.get(key) {
            Some(handle) => handle.sender.send(envelope).map_err(|e| e.0),
            None => Err(envelope),
        }
    }

    fn spawn_if_vacant(&self, id: SM::Id, persisted: Persisted<SM>) {
        use dashmap::mapref::entry::Entry as DEntry;
        match self.entities.entry(id.get_id_string()) {
            DEntry::Occupied(_) => {}
            DEntry::Vacant(slot) => {
                let epoch = next_epoch();
                let (tx, rx) = mpsc::unbounded_channel();
                tokio::spawn(run_entity::<SM>(
                    persisted,
                    rx,
                    EntityContext::new(&id, epoch, Arc::clone(&self.env), Arc::clone(&self.entities)),
                ));
                slot.insert(SoleMailboxHandle { sender: tx, epoch });
            }
        }
    }

    async fn wake(&self, id: &SM::Id) {
        let key = id.get_id_string();
        if self.try_send(&key, Envelope::Drain).is_ok() {
            return;
        }
        if let Err(e) = self.ensure_live(id, &key).await {
            log_transition::<SM>(&format!("sweep wake failed for {key}: {e:#}"));
        }
    }

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
                        generation: loaded.generation,
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

    async fn construct_fresh(&self, id: SM::Id, key: String, construction: SM::Construction) -> anyhow::Result<()> {
        let mut effects = Effects::new(id.clone());
        let state = SM::construct(construction, &mut effects);
        let state_json = serde_json::to_string(&state).map_err(|e| {
            log_transition::<SM>("construct aborted — state failed to serialize");
            anyhow::anyhow!("state failed to serialize: {e}")
        })?;
        let id_json = serde_json::to_string(&id).expect("EntityId serializes");
        let generation = new_generation();
        match store()
            .insert(SM::name(), &key, &id_json, generation, &state_json, tick_deadline::<SM>(&state), &effects.actions)
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
                let persisted = Persisted {
                    state,
                    generation,
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

    pub async fn act_maybe_construct(&self, construction: SM::Construction, action: SM::Action) {
        let id = construction.get_id().clone();
        self.maybe_construct(construction).await;
        self.act(id, action).await;
    }

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
    epoch: u64,
    env: Arc<SM::Env>,
    entities: Arc<DashMap<String, SoleMailboxHandle<SM>>>,
}

impl<SM: StateMachine> EntityContext<SM> {
    fn new(
        id: &SM::Id,
        epoch: u64,
        env: Arc<SM::Env>,
        entities: Arc<DashMap<String, SoleMailboxHandle<SM>>>,
    ) -> Self {
        EntityContext {
            id: id.clone(),
            id_string: id.get_id_string(),
            id_json: serde_json::to_string(id).expect("EntityId serializes"),
            epoch,
            env,
            entities,
        }
    }
}

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
        generation,
        mut version,
        mut next_seq,
    } = persisted;
    let (dispatch_tx, dispatch_rx) = mpsc::unbounded_channel::<Vec<OutboxRow>>();
    let (_dispatcher_stop, stopped) = tokio::sync::watch::channel(());
    tokio::spawn(run_dispatcher(SM::name(), ctx.id_string.clone(), dispatch_rx, stopped));

    drain_outbox::<SM>(&ctx.id_string, &dispatch_tx).await;

    loop {
        let envelope = match SM::schedule(&state) {
            None => rx.recv().await,
            Some(scheduled) => {
                let delay = (scheduled.at - Utc::now())
                    .to_std()
                    .unwrap_or(std::time::Duration::ZERO);

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
                    id_json: ctx.id_json.clone(),
                    state_json,
                    generation,
                    expected_version: version,
                    first_seq: next_seq,
                    next_outbox_seq: next_seq + effects.actions.len() as i64,
                    next_tick_on: tick_deadline::<SM>(&next),
                    outbox: effects.actions,
                    dedup: tracked.as_ref().map(|(token, _)| token.clone()),
                };
                match store().save(&write).await {
                    Ok(SaveOutcome::Ok) => {
                        state = next;
                        version += 1;
                        next_seq = write.next_outbox_seq;
                        if !write.outbox.is_empty() {
                            let _ = dispatch_tx.send(rows_from_drafts(generation, write.first_seq, &write.outbox));
                        }
                        for external in effects.externals {
                            tokio::spawn(external);
                        }
                        if let Some((_, ack)) = tracked {
                            let _ = ack.send(DeliveryOutcome::Applied);
                        }
                    }
                    Ok(SaveOutcome::Conflict { actual }) => {
                        log_transition::<SM>(&format!(
                            "CAS CONFLICT (expected v{version}, store has {actual:?}) — dropping actor; state rebuilds from store"
                        ));
                        ctx.entities
                            .remove_if(&ctx.id_string, |_, current| current.epoch == ctx.epoch);
                        break;
                    }
                    Err(e) => {
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

fn tick_deadline<SM: StateMachine>(state: &SM::State) -> Option<i64> {
    SM::schedule(state).map(|scheduled| scheduled.at.timestamp_millis())
}

fn rows_from_drafts(generation: i64, first_seq: i64, drafts: &[crate::store::OutboxDraft]) -> Vec<OutboxRow> {
    drafts
        .iter()
        .enumerate()
        .map(|(offset, draft)| OutboxRow {
            seq: first_seq + offset as i64,
            sender_generation: generation,
            kind: draft.kind,
            target_machine: draft.target_machine.to_string(),
            target_id_json: draft.target_id_json.clone(),
            payload_json: draft.payload_json.clone(),
        })
        .collect()
}

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
            sender_generation: row.sender_generation,
            seq: row.seq,
        };
        let outcome = match machines().get(&row.target_machine) {
            None => Err(TransientDelivery(format!(
                "no machine named {} registered yet",
                row.target_machine
            ))),
            Some(vtable) => (vtable.deliver)(row.kind, row.target_id_json.clone(), row.payload_json.clone(), token).await,
        };
        match outcome {
            Ok(DeliveryOutcome::Applied | DeliveryOutcome::Duplicate) => {
                ack_with_retry(machine, sender_id, row.sender_generation, row.seq, stopped).await;
                return true;
            }
            Ok(DeliveryOutcome::Rejected(reason)) => {
                println!("[outbox] {machine}/{sender_id} seq {} rejected by {}: {reason}", row.seq, row.target_machine);
                if let Err(e) = store().fail_outbox(machine, sender_id, row.sender_generation, row.seq, &reason).await {
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
    sender_generation: i64,
    seq: i64,
    stopped: &mut tokio::sync::watch::Receiver<()>,
) {
    for attempt in 0..5 {
        match store().ack_outbox(machine, sender_id, sender_generation, seq).await {
            Ok(()) => return,
            Err(e) => {
                println!("[outbox] ack failed for {machine}/{sender_id} seq {seq} (attempt {attempt}): {e:#}");
                if stopped.has_changed().is_err() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

fn log_transition<SM: StateMachine>(label: &str) {
    println!(
        "TRANSITION AT {} - StateMachine: {} - {}",
        Utc::now(),
        SM::name(),
        label
    );
}
