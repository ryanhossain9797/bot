use super::envelope::Envelope;
use super::traits::{EntityId, StateMachine};
use kameo::actor::{ActorRef, Spawn, WeakActorRef};
use kameo::error::{ActorStopReason, Infallible, RegistryError};
use kameo::message::{Context, Message};
use kameo::registry::ACTOR_REGISTRY;
use kameo::Actor;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

// One env per state-machine type, keyed by TypeId. Stored as the concrete `Arc<S::Env>` behind `Any`
// so transition can be handed the concrete env (it needs to read real fields, not a `dyn`).
type EnvMap = HashMap<TypeId, Box<dyn Any + Send + Sync>>;
static ENVS: OnceLock<RwLock<EnvMap>> = OnceLock::new();

fn envs() -> &'static RwLock<EnvMap> {
    ENVS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn register_env<S: StateMachine>(env: S::Env) {
    let boxed: Box<dyn Any + Send + Sync> = Box::new(Arc::new(env));
    envs().write().unwrap().insert(TypeId::of::<S>(), boxed);
}

fn env<S: StateMachine>() -> Arc<S::Env> {
    envs()
        .read()
        .unwrap()
        .get(&TypeId::of::<S>())
        .and_then(|boxed| boxed.downcast_ref::<Arc<S::Env>>())
        .cloned()
        .expect("env not registered — call <StateMachine>::bootstrap() at startup")
}

// The single generic actor. Wraps the pure domain state with the framework's per-entity runtime
// bookkeeping. There is no per-entity concrete type and no macro: this one type is the kameo actor
// for every state machine.
pub struct StateWrapper<S: StateMachine> {
    state: S,
    // Bumped on every arm. Stamped onto the Wakeup the timer fires; a wakeup whose generation no
    // longer matches is from a superseded arm and is ignored.
    generation: u64,
    timer: Option<tokio::task::JoinHandle<()>>,
}

impl<S: StateMachine> Actor for StateWrapper<S> {
    type Args = Self;
    type Error = Infallible;

    async fn on_start(args: Self, _actor_ref: ActorRef<Self>) -> Result<Self, Infallible> {
        Ok(args)
    }

    // Runs on EVERY stop — Delete, panic, or last-ref-drop (the act_maybe_construct loser). Abort the
    // timer and deregister, but deregister BY ID, not by name: remove the entry only if it still holds
    // our id. A blind remove(name) would clobber a successor re-created under the same name, or (for
    // the never-registered loser) clobber the winner. remove_by_id no-ops in both cases.
    async fn on_stop(
        &mut self,
        actor_ref: WeakActorRef<Self>,
        _reason: ActorStopReason,
    ) -> Result<(), Infallible> {
        if let Some(handle) = self.timer.take() {
            handle.abort();
        }
        ACTOR_REGISTRY.lock().unwrap().remove_by_id(&actor_ref.id());
        Ok(())
    }
}

impl<S: StateMachine> Message<Envelope<S::Action>> for StateWrapper<S> {
    type Reply = ();
    async fn handle(&mut self, envelope: Envelope<S::Action>, ctx: &mut Context<Self, ()>) {
        match envelope {
            Envelope::Act(action) => self.dispatch(action),
            Envelope::Wakeup(generation) => self.on_wakeup(generation),
            Envelope::Delete => ctx.stop(), // on_stop aborts the timer and deregisters by id
        }
    }
}

impl<S: StateMachine> StateWrapper<S> {
    fn new(state: S) -> Self {
        StateWrapper {
            state,
            generation: 0,
            timer: None,
        }
    }

    fn id_string(&self) -> String {
        self.state.id().id_string()
    }

    // Value-in / value-out: run the transition on a CLONE, and commit + fire effects ONLY on a valid
    // new state. An Err leaves the live state untouched — no commit, no effects, no re-arm.
    fn dispatch(&mut self, action: S::Action) {
        match self.state.clone().transition(env::<S>(), &action) {
            Ok((next, effects)) => {
                self.state = next;

                let id = self.state.id().id_string();
                for fut in effects.self_actions {
                    let id = id.clone();
                    tokio::spawn(async move {
                        let next = fut.await;
                        let _ = act::<S>(&id, next).await; // loop back to this entity
                    });
                }
                for out in effects.outbound {
                    tokio::spawn(async move {
                        let _ = out.deliver().await; // route to its target entity
                    });
                }

                self.arm();
            }
            Err(e) => {
                println!("[{}] invalid transition, no commit: {e}", self.id_string());
            }
        }
    }

    // A timer fired. Drop it if from a superseded arm; otherwise re-evaluate the schedule against
    // CURRENT state and fire the fresh action only if overdue, else re-arm.
    fn on_wakeup(&mut self, generation: u64) {
        if generation != self.generation {
            println!(
                "[{}] rejected stale wakeup gen={generation} (current={})",
                self.id_string(),
                self.generation
            );
            return;
        }
        match self.state.schedule() {
            Some(scheduled) if scheduled.at <= chrono::Utc::now() => self.dispatch(scheduled.action),
            Some(_) => self.arm(),
            None => {}
        }
    }

    // (Re)arm the single timer from current state: abort any running one, bump the generation, then
    // if the state schedules a self-action, spawn a task that sleeps to the deadline and fires a
    // content-free Wakeup(generation) back. The action is NOT captured — it is re-derived on wake.
    fn arm(&mut self) {
        if let Some(handle) = self.timer.take() {
            handle.abort();
        }
        self.generation += 1;
        let generation = self.generation;
        if let Some(scheduled) = self.state.schedule() {
            let id = self.state.id().id_string();
            let delay = (scheduled.at - chrono::Utc::now())
                .to_std()
                .unwrap_or(std::time::Duration::ZERO);
            self.timer = Some(tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let _ = wake::<S>(&id, generation).await;
            }));
        }
    }
}

// Construct an entity and register it under its id. Idempotent: a lost race (the name got registered
// between our check and ours) surfaces as NameAlreadyRegistered, which we treat as success — the
// just-spawned loser drops here and self-stops, and the existing entity stays live.
pub fn construct<S: StateMachine>(id: S::Id, construction: S::Construction) -> anyhow::Result<()> {
    let key = id.id_string();
    let state = S::construct(id, construction);
    let actor = StateWrapper::<S>::spawn(StateWrapper::new(state));
    match actor.register(key) {
        Ok(()) => Ok(()),
        Err(RegistryError::NameAlreadyRegistered) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub async fn act<S: StateMachine>(id: &str, action: S::Action) -> anyhow::Result<()> {
    match ActorRef::<StateWrapper<S>>::lookup(id)? {
        Some(actor) => actor
            .tell(Envelope::Act(action))
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("tell failed for {id}"))?,
        None => println!("[framework] no actor for {id}; dropping"),
    }
    Ok(())
}

// Get-or-create then act, in one step: look up by id; if absent, construct (idempotent); then act on
// whichever entity exists. This is the only ingestion entry point — it removes the construct→act
// window entirely, since the action always rides along with the create.
pub async fn act_maybe_construct<S: StateMachine>(
    id: S::Id,
    construction: S::Construction,
    action: S::Action,
) -> anyhow::Result<()> {
    let key = id.id_string();
    if ActorRef::<StateWrapper<S>>::lookup(key.as_str())?.is_none() {
        construct::<S>(id, construction)?;
    }
    act::<S>(key.as_str(), action).await
}

async fn wake<S: StateMachine>(id: &str, generation: u64) -> anyhow::Result<()> {
    if let Some(actor) = ActorRef::<StateWrapper<S>>::lookup(id)? {
        actor
            .tell(Envelope::<S::Action>::Wakeup(generation))
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("wake tell failed for {id}"))?;
    }
    Ok(())
}

pub async fn delete<S: StateMachine>(id: &str) -> anyhow::Result<()> {
    if let Some(actor) = ActorRef::<StateWrapper<S>>::lookup(id)? {
        actor
            .tell(Envelope::<S::Action>::Delete)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("delete tell failed for {id}"))?;
    } else {
        println!("[framework] no actor for {id}; nothing to delete");
    }
    Ok(())
}
