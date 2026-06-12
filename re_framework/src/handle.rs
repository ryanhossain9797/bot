use crate::machine::StateMachine;
use chrono::Utc;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const MAILBOX: usize = 64;

// The wire type into an entity's mailbox. Domain actions ride in `Act`; the rest are framework
// meta-actions the runtime delivers through the same channel.
enum Envelope<A> {
    Act(A),
    // A timer firing. Carries no action — the runtime re-evaluates schedule() against current state
    // and fires the fresh action only if overdue. The u64 is the arming GENERATION; a wakeup whose
    // generation no longer matches is from a superseded arm and is dropped.
    Wakeup(u64),
    Delete,
}

// One registry entry: the sender into the entity's task, plus its incarnation — an optimistic-
// concurrency token (cf. a SQL rowversion) so a stopping entity only removes itself if it's still
// the registered occupant, never clobbering a successor re-created under the same id.
struct Entry<SM: StateMachine> {
    sender: mpsc::Sender<Envelope<SM::Action>>,
    incarnation: u64,
}

// The runtime for one state-machine kind: the handle the domain holds (in a OnceLock). Owns the
// entity store (Arc'd, so every clone shares it) and the env. Cloneable.
pub struct StateMachineHandle<SM: StateMachine> {
    entities: Arc<DashMap<SM::Id, Entry<SM>>>,
    env: Arc<SM::Env>,
    next_incarnation: Arc<AtomicU64>,
}

// Hand-written (not derived) so cloning only bumps the Arcs — it must not require SM or SM::Env: Clone.
impl<SM: StateMachine> Clone for StateMachineHandle<SM> {
    fn clone(&self) -> Self {
        StateMachineHandle {
            entities: self.entities.clone(),
            env: self.env.clone(),
            next_incarnation: self.next_incarnation.clone(),
        }
    }
}

impl<SM: StateMachine> StateMachineHandle<SM> {
    pub fn new(env: SM::Env) -> Self {
        StateMachineHandle {
            entities: Arc::new(DashMap::new()),
            env: Arc::new(env),
            next_incarnation: Arc::new(AtomicU64::new(0)),
        }
    }

    // Atomic get-or-create: the DashMap entry lock makes check-and-spawn race-free, so there is no
    // "loser" to clean up. If the id already exists this is a no-op (idempotent — live state is not
    // reset).
    pub async fn maybe_construct(&self, id: SM::Id, construction: SM::Construction) {
        use dashmap::mapref::entry::Entry as DEntry;
        match self.entities.entry(id.clone()) {
            DEntry::Occupied(_) => {}
            DEntry::Vacant(slot) => {
                let incarnation = self.next_incarnation.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = mpsc::channel(MAILBOX);
                let state = SM::construct(id.clone(), construction);
                tokio::spawn(run_entity::<SM>(
                    state,
                    rx,
                    self.env.clone(),
                    self.entities.clone(),
                    id,
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
        // Clone the sender out of the guard, then drop the guard before awaiting — never hold a
        // DashMap shard lock across an await.
        let sender = self.entities.get(&id).map(|e| e.sender.clone());
        if let Some(sender) = sender {
            let _ = sender.send(Envelope::Act(action)).await;
        }
    }

    pub async fn delete(&self, id: SM::Id) {
        let sender = self.entities.get(&id).map(|e| e.sender.clone());
        if let Some(sender) = sender {
            let _ = sender.send(Envelope::Delete).await;
        }
    }
}

// The per-entity task. Owns the state, processes its mailbox serially, runs the value/Result
// transition (commit + effects only on Ok), and manages a single re-evaluating timer. Holds its own
// self-sender for loop-backs and timer wakeups, so self-messaging needs no registry lookup.
async fn run_entity<SM: StateMachine>(
    mut state: SM::State,
    mut rx: mpsc::Receiver<Envelope<SM::Action>>,
    env: Arc<SM::Env>,
    entities: Arc<DashMap<SM::Id, Entry<SM>>>,
    id: SM::Id,
    incarnation: u64,
    self_tx: mpsc::Sender<Envelope<SM::Action>>,
) {
    let mut generation: u64 = 0;
    let mut timer: Option<JoinHandle<()>> = None;
    arm::<SM>(&state, &mut generation, &mut timer, &self_tx);

    while let Some(msg) = rx.recv().await {
        match msg {
            Envelope::Act(action) => {
                match SM::transition(state.clone(), &id, env.clone(), &action) {
                    Ok((next, effects)) => {
                        state = next; // commit — everything below fires only after this, only on Ok
                        for fut in effects.self_actions {
                            let tx = self_tx.clone();
                            tokio::spawn(async move {
                                let action = fut.await;
                                let _ = tx.send(Envelope::Act(action)).await;
                            });
                        }
                        for outbound in effects.outbound {
                            tokio::spawn(outbound); // fire-and-forget (e.g. a cross-machine send)
                        }
                        arm::<SM>(&state, &mut generation, &mut timer, &self_tx);
                    }
                    Err(_e) => { /* invalid (state, action): no commit, no effects, no re-arm */ }
                }
            }
            Envelope::Wakeup(g) => {
                if g != generation {
                    continue; // superseded arm
                }
                match SM::schedule(&state) {
                    Some(s) if s.at <= Utc::now() => {
                        let _ = self_tx.send(Envelope::Act(s.action)).await; // overdue → fire fresh
                    }
                    Some(_) => arm::<SM>(&state, &mut generation, &mut timer, &self_tx), // not yet due
                    None => {}
                }
            }
            Envelope::Delete => break,
        }
    }

    // Cleanup on exit: abort the timer and deregister — but only if we're still the registered
    // occupant (incarnation guard), so we never clobber a successor.
    if let Some(t) = timer.take() {
        t.abort();
    }
    entities.remove_if(&id, |_, e| e.incarnation == incarnation);
}

// (Re)arm the single timer from current state: abort any running one, bump the generation, then if
// the state schedules a self-action, spawn a task that sleeps to the deadline and fires a
// content-free Wakeup(generation) back. The action is NOT captured — it is re-derived on wake.
fn arm<SM: StateMachine>(
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
