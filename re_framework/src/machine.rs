use crate::effects::Effects;
use crate::handle::StateMachineHandle;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::hash::Hash;
use std::sync::Arc;

pub struct Scheduled<A> {
    pub at: chrono::DateTime<chrono::Utc>,
    pub action: A,
}

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
    fn schedule(state: &Self::State) -> Option<Scheduled<Self::Action>>;

    fn handle() -> &'static StateMachineHandle<Self>;
}
