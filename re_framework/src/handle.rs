use crate::effects::Effects;
use crate::machine::{EntityId, Identified, StateMachine};
use crate::store;
use chrono::Utc;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Mailbox message; single-variant for now, kept as an enum for planned control messages.
enum Envelope<A> {
    Act(A),
}

/// Sole sender to a live actor's mailbox; not `Clone` — dropping it (via registry removal) stops the actor (RAII).
struct SoleMailboxHandle<SM: StateMachine> {
    sender: mpsc::UnboundedSender<Envelope<SM::Action>>,
}

impl<SM: StateMachine> SoleMailboxHandle<SM> {
    fn deliver(&self, action: SM::Action) {
        let _ = self.sender.send(Envelope::Act(action));
    }
}

pub struct StateMachineHandle<SM: StateMachine> {
    entities: Arc<DashMap<String, SoleMailboxHandle<SM>>>,
    env: Arc<SM::Env>,
}

impl<SM: StateMachine> StateMachineHandle<SM> {
    pub fn new(env: SM::Env) -> Self {
        StateMachineHandle {
            entities: Arc::new(DashMap::new()),
            env: Arc::new(env),
        }
    }

    pub fn maybe_construct(&self, construction: SM::Construction) {
        use dashmap::mapref::entry::Entry as DEntry;
        let id = construction.get_id().clone();
        match self.entities.entry(id.get_id_string()) {
            DEntry::Occupied(_) => {} // already live — constructed or rehydrated in this process
            DEntry::Vacant(slot) => {
                slot.insert(self.spawn_entry(id, construction));
            }
        }
    }

    // Bring an entity to life and return its sole mailbox. A snapshot on disk means it was
    // already constructed in a past life, so we rehydrate (resume the state — no `construct`,
    // no construct effects); otherwise we construct fresh. Callers never see which happened.
    fn spawn_entry(&self, id: SM::Id, construction: SM::Construction) -> SoleMailboxHandle<SM> {
        let (tx, rx) = mpsc::unbounded_channel();
        let initial = match store::read::<SM>(&id) {
            Some(state) => StoredEntry::Rehydrated(state),
            None => {
                let mut effects = Effects::new(id.clone());
                let state = SM::construct(construction, &mut effects);
                StoredEntry::Constructed { state, effects }
            }
        };
        tokio::spawn(run_entity::<SM>(rx, Arc::clone(&self.env), id, initial));
        SoleMailboxHandle { sender: tx }
    }

    pub fn act(&self, id: SM::Id, action: SM::Action) {
        use dashmap::mapref::entry::Entry as DEntry;
        match self.entities.entry(id.get_id_string()) {
            DEntry::Occupied(slot) => slot.get().deliver(action),
            DEntry::Vacant(_) => eprintln!(
                "[warn] action {action:?} for unconstructed entity {}; dropping (maybe_construct must precede act)",
                id.get_id_string()
            ),
        }
    }

    pub fn delete(&self, id: SM::Id) {
        self.entities.remove(&id.get_id_string());
    }
}

/// How a newly-spawned entity gets its starting state. The store resolves this from the live
/// map and the on-disk snapshot, so the rest of the code never branches on where state came from.
enum StoredEntry<SM: StateMachine> {
    Constructed { state: SM::State, effects: Effects<SM> },
    Rehydrated(SM::State),
}

async fn run_entity<SM: StateMachine>(
    mut rx: mpsc::UnboundedReceiver<Envelope<SM::Action>>,
    env: Arc<SM::Env>,
    id: SM::Id,
    initial: StoredEntry<SM>,
) {
    let mut state = match initial {
        // Fresh: persist the initial state before firing construct effects. On failure, log and
        // carry on with the in-memory state (matches the prior write-only POC behavior).
        StoredEntry::Constructed { state, effects } => {
            match store::write::<SM>(&id, &state) {
                Ok(()) => spawn_effects(effects),
                Err(e) => log_transition::<SM>(&format!(
                    "construct aborted — persistence failed: {e:#}"
                )),
            }
            state
        }
        // Resumed: already on disk, and construct effects belong to the past life that built it.
        StoredEntry::Rehydrated(state) => {
            log_transition::<SM>("rehydrated from disk");
            state
        }
    };

    loop {
        let action = match SM::schedule(&state) {
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

        let Some(Envelope::Act(action)) = action else {
            log_transition::<SM>("Delete");
            break;
        };

        log_transition::<SM>(&format!("Action: {action:?}"));
        let mut effects = Effects::new(id.clone());
        match SM::transition(&state, &id, &env, &action, &mut effects) {
            Ok(next) => match store::write::<SM>(&id, &next) {
                Ok(()) => {
                    state = next;
                    spawn_effects(effects);
                }
                Err(e) => {
                    log_transition::<SM>(&format!("aborted — persistence failed: {e:#}"))
                }
            },
            Err(err) => log_transition::<SM>(&format!("dropped — no state change: {err}")),
        }
    }
}

fn spawn_effects<SM: StateMachine>(effects: Effects<SM>) {
    for outbound in effects.outbound {
        tokio::spawn(outbound);
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
