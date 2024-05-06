use std::{future::Future, pin::Pin, sync::Arc};

use tokio::sync::mpsc::{self, Receiver};

use crate::Env;

pub type TransitionResult<Type, Action> =
    anyhow::Result<(Type, Vec<Pin<Box<dyn Future<Output = Action> + Send>>>)>;

pub type ExternalOperation<Action> = Pin<Box<dyn Future<Output = Action> + Send>>;

#[derive(Clone)]
pub struct Transition<Id, State, Action>(
    pub  fn(
        Arc<Env>,
        Id,
        State,
        Action,
    ) -> Pin<Box<dyn Future<Output = TransitionResult<State, Action>> + Send>>,
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

pub async fn run_entity<
    Id: Clone + Send + 'static,
    Action: Clone + Send + 'static,
    State: Clone + Send + Default,
>(
    env: Arc<Env>,
    id: Id,
    mut receiver: Receiver<Action>,
    handle: LifeCycleHandle<Id, Action>,
    transition: Transition<Id, State, Action>,
) {
    let mut state = State::default();
    while let Some(action) = receiver.recv().await {
        match transition.0(env.clone(), id.clone(), state.clone(), action).await {
            Ok((updated_user, external)) => {
                state = updated_user;
                external.into_iter().for_each(|f| {
                    let handle: LifeCycleHandle<Id, Action> = handle.clone();
                    let user_id = id.clone();
                    tokio::spawn(async move {
                        let action = f.await;
                        handle.act(user_id, action).await;
                    });
                });
            }
            Err(_) => (),
        }
    }
}
