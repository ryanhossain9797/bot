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

pub struct StateWrapper<S: StateMachine> {
    state: S,
    generation: u64,
    timer: Option<tokio::task::JoinHandle<()>>,
}

impl<S: StateMachine> Actor for StateWrapper<S> {
    type Args = Self;
    type Error = Infallible;

    async fn on_start(args: Self, _actor_ref: ActorRef<Self>) -> Result<Self, Infallible> {
        Ok(args)
    }

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
            Envelope::Delete => ctx.stop(), 
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

    fn dispatch(&mut self, action: S::Action) {
        match self.state.clone().transition(env::<S>(), &action) {
            Ok((next, effects)) => {
                self.state = next;

                let id = self.state.id().id_string();
                for fut in effects.self_actions {
                    let id = id.clone();
                    tokio::spawn(async move {
                        let next = fut.await;
                        let _ = act::<S>(&id, next).await; 
                    });
                }
                for out in effects.outbound {
                    tokio::spawn(async move {
                        let _ = out.deliver().await; 
                    });
                }

                self.arm();
            }
            Err(e) => {
                println!("[{}] invalid transition, no commit: {e}", self.id_string());
            }
        }
    }

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
