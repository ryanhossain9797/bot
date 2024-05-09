use std::{future::Future, pin::Pin, sync::Arc};

use tokio::sync::mpsc::{self, Receiver};

pub type TransitionResult<Type, Action> =
    anyhow::Result<(Type, Vec<Pin<Box<dyn Future<Output = Action> + Send>>>)>;

pub type ExternalOperation<Action> = Pin<Box<dyn Future<Output = Action> + Send>>;

pub trait LifeCycleItem: Send + Sync + Clone {}
impl<T: Send + Sync + Clone> LifeCycleItem for T {}

#[derive(Clone)]
pub struct Transition<Id, State, Action, Env>(
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
    Id: LifeCycleItem,
    Action: LifeCycleItem,
{
    pub sender: mpsc::Sender<(Id, Action)>,
}

impl<Id, Action> LifeCycleHandle<Id, Action>
where
    Id: LifeCycleItem + Ord + 'static,
    Action: LifeCycleItem + 'static,
{
    pub async fn act(&self, user_id: Id, user_action: Action) {
        let _ = self
            .sender
            .send((user_id, user_action))
            .await
            .expect("Send failed");
    }
}

pub fn new_life_cycle<
    Id: LifeCycleItem + Ord + 'static,
    State: LifeCycleItem + Default + 'static,
    Action: LifeCycleItem + 'static,
    Env: LifeCycleItem + 'static,
>(
    env: Arc<Env>,
    transition: Transition<Id, State, Action, Env>,
) -> LifeCycleHandle<Id, Action> {
    let (sender, receiver) = mpsc::channel(8);
    let user_life_cycle_handle = LifeCycleHandle { sender };
    tokio::spawn(start_life_cycle(
        env,
        user_life_cycle_handle.clone(),
        receiver,
        transition,
    ));
    user_life_cycle_handle
}

async fn run_entity<
    Id: LifeCycleItem + Ord + 'static,
    State: LifeCycleItem + Default + 'static,
    Action: LifeCycleItem + 'static,
    Env: LifeCycleItem + 'static,
>(
    env: Arc<Env>,
    id: Id,
    mut receiver: Receiver<Action>,
    handle: LifeCycleHandle<Id, Action>,
    transition: Transition<Id, State, Action, Env>,
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

#[derive(Clone)]
pub struct Handle<Action>
where
    Action: LifeCycleItem + 'static,
{
    pub sender: mpsc::Sender<Action>,
}

impl<Action> Handle<Action>
where
    Action: LifeCycleItem + 'static,
{
    pub async fn act(&self, action: Action) {
        let _ = self.sender.send(action).await.expect("Send failed");
    }
}

pub fn new_entity<
    Id: LifeCycleItem + Ord + 'static,
    State: LifeCycleItem + 'static + Default,
    Action: LifeCycleItem + 'static,
    Env: LifeCycleItem + 'static,
>(
    env: Arc<Env>,
    id: Id,
    user_life_cycle_handle: LifeCycleHandle<Id, Action>,
    transition: Transition<Id, State, Action, Env>,
) -> Handle<Action> {
    let (sender, receiver) = mpsc::channel(8);
    tokio::spawn(run_entity(
        env,
        id,
        receiver,
        user_life_cycle_handle,
        transition,
    ));
    Handle { sender }
}

async fn start_life_cycle<
    Id: LifeCycleItem + Ord + 'static,
    State: LifeCycleItem + Default + 'static,
    Action: LifeCycleItem + 'static,
    Env: LifeCycleItem + 'static,
>(
    env: Arc<Env>,
    life_cycle_handle: LifeCycleHandle<Id, Action>,
    mut receiver: Receiver<(Id, Action)>,
    transition: Transition<Id, State, Action, Env>,
) -> ! {
    let mut handle_by_id = std::collections::BTreeMap::<Id, Handle<Action>>::new();

    while let Some((id, action)) = receiver.recv().await {
        match handle_by_id.contains_key(&id) {
            true => (),
            false => {
                let handle = new_entity(
                    env.clone(),
                    id.clone(),
                    life_cycle_handle.clone(),
                    transition.clone(),
                );
                handle_by_id.insert(id.clone(), handle.clone());
            }
        }
        let handle = handle_by_id[&id].clone();
        tokio::spawn(async move { handle.act(action).await });
    }
    panic!()
}
