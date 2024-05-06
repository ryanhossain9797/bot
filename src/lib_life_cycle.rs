use std::{future::Future, pin::Pin, sync::Arc};

use tokio::sync::mpsc;

use crate::Env;

pub type TransitionResult<Type, Action> =
    anyhow::Result<(Type, Vec<Pin<Box<dyn Future<Output = Action> + Send>>>)>;

pub type ExternalOperation<Action> = Pin<Box<dyn Future<Output = Action> + Send>>;

#[derive(Clone)]
pub struct Transition<Id, Data, Action>(
    pub  fn(
        Arc<Env>,
        Id,
        Data,
        Action,
    ) -> Pin<Box<dyn Future<Output = TransitionResult<Data, Action>> + Send>>,
);

#[derive(Clone)]
pub struct LifeCycleHandle<Id, Action>
where
    Id: Clone,
    Action: Clone,
{
    pub sender: mpsc::Sender<(Id, Action)>,
}

impl<Id, Action> LifeCycleHandle<Id, Action>
where
    Id: Clone,
    Action: Clone,
{
    pub async fn act(&self, user_id: Id, user_action: Action) {
        let _ = self
            .sender
            .send((user_id, user_action))
            .await
            .expect("Send failed");
    }
}
