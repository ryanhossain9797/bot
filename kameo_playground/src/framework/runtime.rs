use super::envelope::Envelope;
use super::traits::{Entity, EntityId, Env, StateMachine};
use kameo::actor::{RemoteActorRef, Spawn};
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

// One env per state-machine type, keyed by TypeId, held as a trait object (no Any, no downcast).
static ENVS: OnceLock<RwLock<HashMap<TypeId, Arc<dyn Env>>>> = OnceLock::new();

fn envs() -> &'static RwLock<HashMap<TypeId, Arc<dyn Env>>> {
    ENVS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn register_env<S: StateMachine>(env: S::Env) {
    let env: Arc<dyn Env> = Arc::new(env);
    envs().write().unwrap().insert(TypeId::of::<S>(), env);
}

pub fn env<S: StateMachine>() -> Arc<dyn Env> {
    envs()
        .read()
        .unwrap()
        .get(&TypeId::of::<S>())
        .cloned()
        .expect("env not registered — call <StateMachine>::bootstrap() at startup")
}

pub struct StateWrapper<S> {
    state: S,
    // Bumped on every arm. Stamped onto the Wakeup the timer fires; a wakeup whose generation no
    // longer matches is from a superseded arm and is ignored.
    generation: u64,
    timer: Option<tokio::task::JoinHandle<()>>,
}

impl<S: StateMachine> StateWrapper<S> {
    pub fn new(state: S) -> Self {
        StateWrapper {
            state,
            generation: 0,
            timer: None,
        }
    }
    pub fn id_string(&self) -> String {
        self.state.id().id_string()
    }
    pub fn dispatch(&mut self, action: S::Action) {
        let env = env::<S>();
        let effects = self.state.transition(env.as_ref(), action);

        // Post-transition tasks: spawned detached, so they run AFTER this returns. Their results
        // loop back in as new messages.
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

    // Framework teardown before the actor stops: kill the timer and remove the DHT name so later
    // lookups miss cleanly instead of resolving to a dead actor.
    pub async fn teardown(&mut self) {
        if let Some(handle) = self.timer.take() {
            handle.abort();
        }
        let _ = kameo::remote::unregister(self.state.id().id_string()).await;
    }

    // A timer fired. If the wakeup is from a superseded arm, drop it; otherwise re-evaluate the
    // schedule against CURRENT state and fire the fresh action only if its deadline is overdue. If
    // the deadline moved out (state changed), just re-arm; if nothing is scheduled, stop.
    pub fn on_wakeup(&mut self, generation: u64) {
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

    // (Re)arm the single timer from the current state: abort any running one, bump the generation,
    // then if the state schedules a self-action, spawn a task that sleeps until the deadline and
    // fires a content-free Wakeup(generation) back. The JoinHandle is live-only (never serialized);
    // the action itself is NOT captured — it is re-derived when the wakeup lands.
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

// Boot the actor runtime (swarm). Per-state-machine env is registered via <SM>::bootstrap().
pub fn bootstrap() -> anyhow::Result<()> {
    kameo::remote::bootstrap().map_err(|e| anyhow::anyhow!("bootstrap failed: {e}"))?;
    Ok(())
}

pub async fn construct<S: StateMachine>(
    id: S::Id,
    construction: S::Construction,
) -> anyhow::Result<()> {
    let entity = S::Wrapped::build(id, construction);
    let key = entity.id_string();
    S::Wrapped::spawn(entity).register(key).await?;
    Ok(())
}

pub async fn act<S: StateMachine>(id: &str, action: S::Action) -> anyhow::Result<()> {
    match RemoteActorRef::<S::Wrapped>::lookup(id).await? {
        Some(entity) => entity.tell(&Envelope::Act(action)).send_ack().await?,
        None => println!("[framework] no actor for {id}; dropping"),
    }
    Ok(())
}

async fn wake<S: StateMachine>(id: &str, generation: u64) -> anyhow::Result<()> {
    if let Some(entity) = RemoteActorRef::<S::Wrapped>::lookup(id).await? {
        entity
            .tell(&Envelope::<S::Action>::Wakeup(generation))
            .send_ack()
            .await?;
    }
    Ok(())
}

pub async fn delete<S: StateMachine>(id: &str) -> anyhow::Result<()> {
    match RemoteActorRef::<S::Wrapped>::lookup(id).await? {
        Some(entity) => entity.tell(&Envelope::<S::Action>::Delete).send_ack().await?,
        None => println!("[framework] no actor for {id}; nothing to delete"),
    }
    Ok(())
}
