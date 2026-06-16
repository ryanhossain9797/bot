use crate::effects::Effects;
use crate::machine::{EntityId, Identified, StateMachine};
use crate::persistence::{delete_state, load_state, persist_state};
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

    fn entry(&self, id: &SM::Id) -> dashmap::mapref::entry::Entry<'_, String, SoleMailboxHandle<SM>> {
        use dashmap::mapref::entry::Entry as DEntry;
        let key = id.get_id_string();
        match self.entities.entry(key.clone()) {
            DEntry::Vacant(slot) => match load_state::<SM>(id) {
                Some(state) => {
                    let (tx, rx) = mpsc::unbounded_channel();
                    tokio::spawn(run_entity::<SM>(state, rx, Arc::clone(&self.env), id.clone()));
                    slot.insert(SoleMailboxHandle { sender: tx });
                    self.entities.entry(key)
                }
                None => DEntry::Vacant(slot),
            },
            occupied => occupied,
        }
    }

    pub fn maybe_construct(&self, construction: SM::Construction) {
        use dashmap::mapref::entry::Entry as DEntry;
        let id = construction.get_id().clone();
        match self.entry(&id) {
            DEntry::Occupied(_) => {}
            DEntry::Vacant(slot) => {
                let (tx, rx) = mpsc::unbounded_channel();
                let mut effects = Effects::new(id.clone());
                let state = SM::construct(construction, &mut effects);
                match post_transition::<SM>(&id, &state, effects) {
                    Ok(()) => {
                        slot.insert(SoleMailboxHandle { sender: tx });
                        tokio::spawn(run_entity::<SM>(state, rx, Arc::clone(&self.env), id));
                    }
                    Err(e) => {
                        log_transition::<SM>(&format!("construct aborted — persistence failed: {e:#}"))
                    }
                }
            }
        }
    }

    pub fn act(&self, id: SM::Id, action: SM::Action) {
        use dashmap::mapref::entry::Entry as DEntry;
        match self.entry(&id) {
            DEntry::Occupied(slot) => slot.get().deliver(action),
            DEntry::Vacant(_) => eprintln!(
                "[warn] action {action:?} for unconstructed entity {}; dropping (maybe_construct must precede act)",
                id.get_id_string()
            ),
        }
    }

    pub fn delete(&self, id: SM::Id) {
        use dashmap::mapref::entry::Entry as DEntry;
        if let DEntry::Occupied(slot) = self.entities.entry(id.get_id_string()) {
            if let Err(e) = delete_state::<SM>(&id) {
                log_transition::<SM>(&format!("delete — {e:#}"));
            }
            slot.remove();
        }
    }
}

async fn run_entity<SM: StateMachine>(
    mut state: SM::State,
    mut rx: mpsc::UnboundedReceiver<Envelope<SM::Action>>,
    env: Arc<SM::Env>,
    id: SM::Id,
) {
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
            Ok(next) => match post_transition::<SM>(&id, &next, effects) {
                Ok(()) => state = next,
                Err(e) => log_transition::<SM>(&format!("aborted — persistence failed: {e:#}")),
            },
            Err(err) => log_transition::<SM>(&format!("dropped — no state change: {err}")),
        }
    }
}

fn post_transition<SM: StateMachine>(
    id: &SM::Id,
    state: &SM::State,
    effects: Effects<SM>,
) -> anyhow::Result<()> {
    persist_state::<SM>(id, state)?;
    spawn_effects(effects);
    Ok(())
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
