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

#[derive(Default)]
struct Core {
    acts: u64,
    timer: Option<tokio::task::JoinHandle<()>>,
}

pub struct StateWrapper<S> {
    state: S,
    core: Core,
}

impl<S: StateMachine> StateWrapper<S> {
    pub fn new(state: S) -> Self {
        StateWrapper {
            state,
            core: Core::default(),
        }
    }
    pub fn id_string(&self) -> String {
        self.state.id().id_string()
    }
    pub fn dispatch(&mut self, action: S::Action) {
        let env = env::<S>();
        self.core.acts += 1;
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

    // (Re)arm the single timer from the current state: abort any running one, then if the state
    // schedules a self-action, spawn a task that sleeps and fires it back. The JoinHandle lives in
    // Core (live-only, never serialized).
    fn arm(&mut self) {
        if let Some(handle) = self.core.timer.take() {
            handle.abort();
        }
        if let Some(scheduled) = self.state.schedule() {
            let id = self.state.id().id_string();
            self.core.timer = Some(tokio::spawn(async move {
                tokio::time::sleep(scheduled.after).await;
                let _ = act::<S>(&id, scheduled.action).await;
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
        Some(entity) => entity.tell(&action).send()?,
        None => println!("[framework] no actor for {id}; dropping"),
    }
    Ok(())
}
