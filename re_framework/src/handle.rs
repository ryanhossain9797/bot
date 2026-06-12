use crate::machine::{EntityId, Identified, StateMachine};
use chrono::Utc;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const MAILBOX: usize = 64;

enum Envelope<A> {
    Act(A),
    Wakeup(u64),
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
        let key = construction.get_id().get_id_string().to_string();
        match self.entities.entry(key.clone()) {
            DEntry::Occupied(_) => {}
            DEntry::Vacant(slot) => {
                let incarnation = self.next_incarnation.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = mpsc::channel(MAILBOX);
                let state = SM::construct(construction);
                assert!(
                    state.get_id().get_id_string() == key,
                    "construct produced a state whose id disagrees with the constructor"
                );
                tokio::spawn(run_entity::<SM>(
                    state,
                    rx,
                    self.env.clone(),
                    self.entities.clone(),
                    key,
                    incarnation,
                    tx.clone(),
                ));
                slot.insert(Entry {
                    sender: tx,
                    incarnation,
                });
            }
        }
    }

    pub async fn act(&self, id: SM::Id, action: SM::Action) {
        let sender = self.entities.get(id.get_id_string()).map(|e| e.sender.clone());
        if let Some(sender) = sender {
            let _ = sender.send(Envelope::Act(action)).await;
        }
    }

    pub async fn delete(&self, id: SM::Id) {
        let sender = self.entities.get(id.get_id_string()).map(|e| e.sender.clone());
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
    id_string: String,
    incarnation: u64,
    self_tx: mpsc::Sender<Envelope<SM::Action>>,
) {
    let mut generation: u64 = 0;
    let mut timer: Option<JoinHandle<()>> = None;
    reschedule_timer::<SM>(&state, &mut generation, &mut timer, &self_tx);

    while let Some(msg) = rx.recv().await {
        match msg {
            Envelope::Act(action) => {
                match SM::transition(&state, &env, &action) {
                    Ok((next, effects)) => {
                        state = next;
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
                        reschedule_timer::<SM>(&state, &mut generation, &mut timer, &self_tx);
                    }
                    Err(_e) => {}
                }
            }
            Envelope::Wakeup(g) => {
                if g != generation {
                    continue;
                }
                match SM::schedule(&state) {
                    Some(s) if s.at <= Utc::now() => {
                        let _ = self_tx.send(Envelope::Act(s.action)).await;
                    }
                    Some(_) => reschedule_timer::<SM>(&state, &mut generation, &mut timer, &self_tx),
                    None => {}
                }
            }
            Envelope::Delete => break,
        }
    }

    if let Some(t) = timer.take() {
        t.abort();
    }
    entities.remove_if(&id_string, |_, e| e.incarnation == incarnation);
}

fn reschedule_timer<SM: StateMachine>(
    state: &SM::State,
    generation: &mut u64,
    timer: &mut Option<JoinHandle<()>>,
    self_tx: &mpsc::Sender<Envelope<SM::Action>>,
) {
    if let Some(t) = timer.take() {
        t.abort();
    }
    *generation += 1;
    let g = *generation;
    if let Some(scheduled) = SM::schedule(state) {
        let tx = self_tx.clone();
        let delay = (scheduled.at - Utc::now())
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);
        *timer = Some(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(Envelope::Wakeup(g)).await;
        }));
    }
}
