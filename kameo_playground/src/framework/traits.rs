use super::effects::Effects;
use super::envelope::Envelope;
use kameo::message::Message;
use kameo::remote::{RemoteActor, RemoteMessage};
use kameo::Actor;
use serde::de::DeserializeOwned;
use serde::Serialize;

// An id type must be convertible to its registry-key string.
pub trait EntityId {
    fn id_string(&self) -> String;
}

// The framework provides this TRAIT (the interface), not a concrete env. Each state machine's env
// type implements it. Held as `Arc<dyn Env>` — no `Any`, no downcast. (get_config is a placeholder.)
pub trait Env: Send + Sync + 'static {
    fn get_config(&self);
}

// A scheduled self-action: an ABSOLUTE deadline `at` (derived from stored state, so re-evaluating
// schedule() yields the same instant rather than resetting the clock) plus the action to run once it
// is overdue.
pub struct Scheduled<A> {
    pub at: chrono::DateTime<chrono::Utc>,
    pub action: A,
}

// Implemented by the PURE domain state. The env type is the DOMAIN's own (associated `type Env`,
// bounded by the framework's `Env` trait); the framework holds it as `dyn Env` and hands transitions
// a `&dyn Env`.
pub trait StateMachine: Sized + 'static {
    type Id: EntityId + Send + 'static;
    type Action: Serialize + DeserializeOwned + Send + Sync + 'static;
    type Construction;
    type Env: Env;
    type Wrapped: Entity<State = Self>;
    fn build_env() -> anyhow::Result<Self::Env>;
    fn construct(id: Self::Id, construction: Self::Construction) -> Self;
    fn id(&self) -> &Self::Id;
    fn transition(&mut self, env: &dyn Env, action: Self::Action) -> Effects<Self>;
    // The next self-action to fire on a timer (pure over state; re-evaluated after each transition).
    fn schedule(&self) -> Option<Scheduled<Self::Action>>;

    // Build + register this state machine's env into the per-type registry. Call once at startup.
    fn bootstrap() -> anyhow::Result<()> {
        super::runtime::register_env::<Self>(Self::build_env()?);
        Ok(())
    }
}

// The kameo-facing wrapper contract — satisfied by the type the `entity!` macro generates. Carries
// the kameo requirements as supertraits so the runtime needs only `S: StateMachine`.
pub trait Entity:
    Actor<Args = Self>
    + RemoteActor
    + Message<Envelope<<Self::State as StateMachine>::Action>>
    + RemoteMessage<Envelope<<Self::State as StateMachine>::Action>>
    + Sized
{
    type State: StateMachine;
    fn build(
        id: <Self::State as StateMachine>::Id,
        construction: <Self::State as StateMachine>::Construction,
    ) -> Self;
    fn id_string(&self) -> String;
}
