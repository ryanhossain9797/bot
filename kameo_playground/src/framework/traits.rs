use super::effects::Effects;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Arc;

// An id type must be convertible to its registry-key string.
pub trait EntityId {
    fn id_string(&self) -> String;
}

// A scheduled self-action: an ABSOLUTE deadline `at` (derived from stored state, so re-evaluating
// schedule() yields the same instant rather than resetting the clock) plus the action to run once it
// is overdue.
pub struct Scheduled<A> {
    pub at: chrono::DateTime<chrono::Utc>,
    pub action: A,
}

// Implemented by the PURE domain state. `transition` is value-in / value-out and fallible: it takes
// the current state by value and returns the next state plus its effects, or an error. The runtime
// runs it on a CLONE and commits (and fires effects) ONLY on Ok — an Err leaves the live state
// untouched, so an invalid (state, action) pair is a clean no-op.
// State, Id, and Action carry Serialize + DeserializeOwned for persistence (#106): state + the
// outbox round-trip durable storage. NOT for transport — local messaging moves values, no wire.
// Env is deliberately exempt: it holds live, unserializable handles rebuilt at startup, never
// persisted. (Sync is also gone — that was a remote-only requirement.)
pub trait StateMachine: Sized + Clone + Serialize + DeserializeOwned + Send + 'static {
    type Id: EntityId + Clone + Serialize + DeserializeOwned + Send + 'static;
    type Action: Serialize + DeserializeOwned + Send + 'static;
    type Construction: Send + 'static;
    type Env: Send + Sync + 'static;
    fn build_env() -> anyhow::Result<Self::Env>;
    fn construct(id: Self::Id, construction: Self::Construction) -> Self;
    fn id(&self) -> &Self::Id;
    fn transition(
        self,
        env: Arc<Self::Env>,
        action: &Self::Action,
    ) -> anyhow::Result<(Self, Effects<Self>)>;
    // The next self-action to fire on a timer (pure over state; re-evaluated after each transition).
    fn schedule(&self) -> Option<Scheduled<Self::Action>>;

    // Build + register this state machine's env into the per-type registry. Call once at startup.
    fn bootstrap() -> anyhow::Result<()> {
        super::runtime::register_env::<Self>(Self::build_env()?);
        Ok(())
    }
}
