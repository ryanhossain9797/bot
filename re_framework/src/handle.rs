use crate::effects::Effects;
use crate::machine::{EntityId, Identified, StateMachine};
use chrono::Utc;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

const MAILBOX: usize = 64;

enum Envelope<A> {
    Act(A),
    Delete,
}

struct Entry<SM: StateMachine> {
    sender: mpsc::Sender<Envelope<SM::Action>>,
    incarnation: u64,
}

pub struct StateMachineHandle<SM: StateMachine> {
    entities: Arc<DashMap<String, Entry<SM>>>,
    env: Arc<SM::Env>,
    next_incarnation: Arc<AtomicU64>,
}

impl<SM: StateMachine> StateMachineHandle<SM> {
    pub fn new(env: SM::Env) -> Self {
        StateMachineHandle {
            entities: Arc::new(DashMap::new()),
            env: Arc::new(env),
            next_incarnation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn maybe_construct(&self, construction: SM::Construction) {
        use dashmap::mapref::entry::Entry as DEntry;
        let id = construction.get_id().clone();
        match self.entities.entry(id.get_id_string()) {
            DEntry::Occupied(_) => {}
            DEntry::Vacant(slot) => {
                let incarnation = self.next_incarnation.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = mpsc::channel(MAILBOX);
                let (state, effects) = SM::construct(construction);
                tokio::spawn(run_entity::<SM>(
                    state,
                    rx,
                    self.env.clone(),
                    self.entities.clone(),
                    id,
                    incarnation,
                    tx.clone(),
                    effects,
                ));
                slot.insert(Entry {
                    sender: tx,
                    incarnation,
                });
            }
        }
    }

    pub async fn act(&self, id: SM::Id, action: SM::Action) {
        let sender = self
            .entities
            .get(&id.get_id_string())
            .map(|e| e.sender.clone());
        match sender {
            Some(sender) => {
                let _ = sender.send(Envelope::Act(action)).await;
            }
            None => eprintln!(
                "[warn] action {action:?} for unconstructed entity {}; dropping (maybe_construct must precede act)",
                id.get_id_string()
            ),
        }
    }

    pub async fn delete(&self, id: SM::Id) {
        let sender = self
            .entities
            .get(&id.get_id_string())
            .map(|e| e.sender.clone());
        if let Some(sender) = sender {
            let _ = sender.send(Envelope::Delete).await;
        }
    }
}

async fn run_entity<SM: StateMachine>(
    mut state: SM::State,
    mut rx: mpsc::Receiver<Envelope<SM::Action>>,
    env: Arc<SM::Env>,
    entities: Arc<DashMap<String, Entry<SM>>>,
    id: SM::Id,
    incarnation: u64,
    self_tx: mpsc::Sender<Envelope<SM::Action>>,
    initial: Effects<SM>,
) {
    spawn_effects::<SM>(initial, &self_tx);

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

        let action = match action {
            Some(Envelope::Act(action)) => action,
            Some(Envelope::Delete) | None => {
                log_transition::<SM>("Delete");
                break;
            }
        };

        log_transition::<SM>(&format!("Action: {action:?}"));
        if let Ok((next, effects)) = SM::transition(&state, &id, &env, &action) {
            state = next;
            spawn_effects::<SM>(effects, &self_tx);
        }
    }

    entities.remove_if(&id.get_id_string(), |_, e| e.incarnation == incarnation);
}

fn spawn_effects<SM: StateMachine>(
    effects: Effects<SM>,
    self_tx: &mpsc::Sender<Envelope<SM::Action>>,
) {
    for fut in effects.self_actions {
        let tx = self_tx.clone();
        tokio::spawn(async move {
            let action = fut.await;
            let _ = tx.send(Envelope::Act(action)).await;
        });
    }
    for outbound in effects.outbound {
        tokio::spawn(outbound);
    }
}

fn log_transition<SM: StateMachine>(label: &str) {
    println!(
        "TRANSITION AT {} - StateMachine: {} - {}",
        Utc::now(),
        std::any::type_name::<SM::State>()
            .rsplit("::")
            .next()
            .unwrap_or("?"),
        label
    );
}
