use crate::effects::Effects;
use crate::handle::StateMachineHandle;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Arc;

pub trait EntityId: Clone + Eq + Serialize + DeserializeOwned + Send + Sync + 'static {
    fn get_id_string(&self) -> String;
}

pub trait Identified {
    type Id: EntityId;
    fn get_id(&self) -> &Self::Id;
}

pub struct Scheduled<A> {
    pub at: chrono::DateTime<chrono::Utc>,
    pub action: A,
}

pub trait StateMachine: Sized + 'static {
    type State: Clone + Serialize + DeserializeOwned + Send + 'static;
    type Id: EntityId;
    type Action: Serialize + DeserializeOwned + Send + std::fmt::Debug + 'static;
    type Construction: Identified<Id = Self::Id> + Send + 'static;
    type Env: Send + Sync + 'static;

    fn construct(construction: Self::Construction, effects: &mut Effects<Self>) -> Self::State;
    fn transition(
        state: &Self::State,
        id: &Self::Id,
        env: &Arc<Self::Env>,
        action: &Self::Action,
        effects: &mut Effects<Self>,
    ) -> anyhow::Result<Self::State>;
    fn schedule(state: &Self::State) -> Option<Scheduled<Self::Action>>;
    fn handle() -> &'static StateMachineHandle<Self>;

    /// Persisted identity: keys `entities`/`outbox`/`call_dedup` rows and routes outbox
    /// dispatch. Must be explicit and stable — renaming the Rust type must not change it.
    fn name() -> &'static str;
}
