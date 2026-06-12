use crate::effects::Effects;
use crate::handle::StateMachineHandle;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::hash::Hash;
use std::sync::Arc;

// A scheduled self-action: an ABSOLUTE deadline `at` (derived from stored state, so re-evaluating
// schedule() yields the same instant rather than resetting the clock) plus the action to run once it
// is overdue.
pub struct Scheduled<A> {
    pub at: chrono::DateTime<chrono::Utc>,
    pub action: A,
}

// The DEFINITION of a state machine. Implemented by a dedicated "glue" type (e.g.
// `ConversationMachine`), NOT by the state itself — the state is plain serializable data carried in
// the associated `State` type; behaviour and the type-wiring live here.
//
// `transition` is value-in / value-out and fallible: the runtime runs it on a CLONE of the state and
// commits (and fires effects) ONLY on Ok — an Err leaves the live state untouched, so an invalid
// (state, action) pair is a clean no-op. Action staleness is handled here, as domain logic: a stale
// action simply doesn't match the current state and falls through to Err. The `id` is passed in (the
// state need not store it).
//
// State, Id, and Action carry Serialize + DeserializeOwned for persistence (#106). Env does NOT — it
// holds live, unserializable handles rebuilt at startup. Id is Sync because it keys the shared
// registry `DashMap` (a concurrent-map requirement, unrelated to transport).
pub trait StateMachine: Sized + 'static {
    type State: Clone + Serialize + DeserializeOwned + Send + 'static;
    type Id: Clone + Eq + Hash + Serialize + DeserializeOwned + Send + Sync + 'static;
    type Action: Serialize + DeserializeOwned + Send + 'static;
    type Construction: Send + 'static;
    type Env: Send + Sync + 'static;

    fn construct(id: Self::Id, construction: Self::Construction) -> Self::State;
    fn transition(
        state: Self::State,
        id: &Self::Id,
        env: Arc<Self::Env>,
        action: &Self::Action,
    ) -> anyhow::Result<(Self::State, Effects<Self>)>;
    // The next self-action to fire on a timer (pure over state; re-evaluated after each transition).
    fn schedule(state: &Self::State) -> Option<Scheduled<Self::Action>>;

    // This machine kind's global handle. Every state machine is a globally-addressable singleton: the
    // domain builds the handle once at startup and stashes it in a `OnceLock`, returning it here.
    // The framework calls this to route cross-machine sends (`Effects::send::<T>`).
    fn handle() -> &'static StateMachineHandle<Self>;
}
