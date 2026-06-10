use kameo::actor::{RemoteActorRef, Spawn};
use kameo::message::Message;
use kameo::remote::{RemoteActor, RemoteMessage};
use kameo::Actor;
use serde::Serialize;

// The id type must be convertible to its registry-key string.
pub trait EntityId {
    fn id_string(&self) -> String;
}

// Framework bookkeeping wrapped into every entity. Fields are PRIVATE to this module, so only
// framework code can read/mutate them — domain code can store a `Core` and hand back a reference,
// but cannot touch its internals. Real framework would hold env, the scheduler timer handle, the
// pending buffer, last_transition, etc. Here: a count of actions handled.
#[derive(Default)]
pub struct Core {
    acts: u64,
}

pub trait Entity:
    Actor<Args = Self> + RemoteActor + Message<Self::Action> + RemoteMessage<Self::Action> + Sized
{
    type Id: EntityId;
    type Action: Serialize + Send + 'static;
    type Construction;
    fn construct(id: Self::Id, construction: Self::Construction) -> Self;
    // Storage accessors: the entity owns a `Core`; these let the framework reach it. The domain
    // can't *use* what it returns — `Core`'s fields are private to this module.
    fn get_core(&self) -> &Core;
    fn with_core(&mut self) -> &mut Core;
    fn transition(&mut self, action: Self::Action);
}

pub async fn construct<E: Entity>(id: E::Id, construction: E::Construction) -> anyhow::Result<()> {
    let key = id.id_string();
    E::spawn(E::construct(id, construction)).register(key).await?;
    Ok(())
}

pub async fn act<E: Entity>(id: &str, action: E::Action) -> anyhow::Result<()> {
    match RemoteActorRef::<E>::lookup(id).await? {
        Some(entity) => entity.tell(&action).send()?,
        None => println!("[lifecycle] no actor for {id}; dropping"),
    }
    Ok(())
}

// Per-message framework runner the concrete handler delegates to: bookkeeping on Core, then the
// entity's own transition. Generic — reused by every entity.
pub fn run<E: Entity>(entity: &mut E, action: E::Action) {
    entity.with_core().acts += 1;
    let n = entity.get_core().acts;
    println!("[fw] act #{n}");
    entity.transition(action);
}
