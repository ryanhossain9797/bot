use super::effects::Effects;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Arc;

pub trait EntityId {
    fn id_string(&self) -> String;
}

pub struct Scheduled<A> {
    pub at: chrono::DateTime<chrono::Utc>,
    pub action: A,
}

pub trait StateMachine: Sized + Clone + Serialize + DeserializeOwned + Send + 'static {
    type Id: EntityId + Clone + Serialize + DeserializeOwned + Send + 'static;
    type Action: Serialize + DeserializeOwned + Send + 'static;
    type Construction: Send + 'static;
    type Env: Send + Sync + 'static;
    fn construct(id: Self::Id, construction: Self::Construction) -> Self;
    fn id(&self) -> &Self::Id;
    fn transition(
        self,
        env: Arc<Self::Env>,
        action: &Self::Action,
    ) -> anyhow::Result<(Self, Effects<Self>)>;
    fn schedule(&self) -> Option<Scheduled<Self::Action>>;
}
