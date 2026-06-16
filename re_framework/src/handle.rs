use crate::effects::Effects;
use crate::machine::{EntityId, Identified, StateMachine};
use crate::store;
use anyhow::Context;
use chrono::Utc;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Mailbox message; single-variant for now, kept as an enum for planned control messages.
pub(crate) enum Envelope<A> {
    Act(A),
}

/// Sole sender to a live actor's mailbox; not `Clone` — dropping it (via registry removal) stops the actor (RAII).
pub(crate) struct SoleMailboxHandle<SM: StateMachine> {
    pub(crate) sender: mpsc::UnboundedSender<Envelope<SM::Action>>,
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
        match store::entry(&self.entities, id.get_id_string()) {
            DEntry::Occupied(_) => {}
            DEntry::Vacant(slot) => {
                store::spawn_entity(slot, construction, Arc::clone(&self.env), id);
            }
        }
    }

    pub fn act(&self, id: SM::Id, action: SM::Action) {
        use dashmap::mapref::entry::Entry as DEntry;
        match store::entry(&self.entities, id.get_id_string()) {
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

pub(crate) async fn run_entity<SM: StateMachine>(
    mut state: SM::State,
    mut rx: mpsc::UnboundedReceiver<Envelope<SM::Action>>,
    env: Arc<SM::Env>,
    id: SM::Id,
    initial: Effects<SM>,
) {
    match persist_state::<SM>(&id, &state) {
        Ok(()) => spawn_effects(initial),
        Err(e) => log_transition::<SM>(&format!("construct aborted — persistence failed: {e:#}")),
    }

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
            Ok(next) => match persist_state::<SM>(&id, &next) {
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

// POC: write the latest state to framework_db/<state machine>/<entity id>.json on every transition.
// Write-only for now — nothing reads it back yet. On failure the caller aborts the transition.
fn persist_state<SM: StateMachine>(id: &SM::Id, state: &SM::State) -> anyhow::Result<()> {
    let dir = std::path::Path::new("framework_db").join(SM::name());
    std::fs::create_dir_all(&dir).with_context(|| format!("create_dir_all {}", dir.display()))?;
    let safe_id: String = id
        .get_id_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    let path = dir.join(format!("{safe_id}.json"));
    let tmp = dir.join(format!("{safe_id}.json.tmp"));
    let bytes = serde_json::to_vec_pretty(state).context("serialize state")?;
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

fn log_transition<SM: StateMachine>(label: &str) {
    println!(
        "TRANSITION AT {} - StateMachine: {} - {}",
        Utc::now(),
        SM::name(),
        label
    );
}
