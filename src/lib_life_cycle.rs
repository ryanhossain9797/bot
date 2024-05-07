use std::{future::Future, marker::PhantomData, pin::Pin, sync::Arc};

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
pub struct LifeCycleHandle<Id, State, Action>
where
    Id: Clone + Send + Sync,
    State: Clone + Send + Sync + Default,
    Action: Clone + Send,
{
    pub state_type: PhantomData<State>,
    pub sender: mpsc::Sender<(Id, Action)>,
}

impl<Id, State, Action> LifeCycleHandle<Id, State, Action>
where
    Id: Clone + Send + Sync + Ord + 'static,
    State: Clone + Send + Sync + Default + 'static,
    Action: Clone + Send + Sync + 'static,
{
    pub fn new(env: Arc<Env>, transition: Transition<Id, State, Action>) -> Self {
        let (sender, receiver) = mpsc::channel(8);
        let user_life_cycle_handle = Self {
            state_type: PhantomData,
            sender,
        };
        tokio::spawn(start_life_cycle(
            env,
            user_life_cycle_handle.clone(),
            receiver,
            transition,
        ));
        user_life_cycle_handle
    }
    pub async fn act(&self, user_id: Id, user_action: Action) {
        let _ = self
            .sender
            .send((user_id, user_action))
            .await
            .expect("Send failed");
    }
}

pub async fn run_entity<
    Id: Clone + Send + Sync + Ord + 'static,
    Action: Clone + Send + Sync + 'static,
    State: Clone + Send + Sync + Default + 'static,
>(
    env: Arc<Env>,
    id: Id,
    mut receiver: Receiver<Action>,
    handle: LifeCycleHandle<Id, State, Action>,
    transition: Transition<Id, State, Action>,
) {
    let mut state = State::default();
    while let Some(action) = receiver.recv().await {
        match transition.0(env.clone(), id.clone(), state.clone(), action).await {
            Ok((updated_user, external)) => {
                state = updated_user;
                external.into_iter().for_each(|f| {
                    let handle: LifeCycleHandle<Id, State, Action> = handle.clone();
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
pub struct Handle<Id, State, Action>
where
    Id: Clone + Send + 'static,
    State: Clone + Send + 'static + Default,
    Action: Clone + Send + 'static,
{
    id_type: PhantomData<Id>,
    state_type: PhantomData<State>,
    pub sender: mpsc::Sender<Action>,
}
impl<Id, State, Action> Handle<Id, State, Action>
where
    Id: Clone + Send + Sync + Ord + 'static,
    State: Clone + Send + Sync + 'static + Default,
    Action: Clone + Send + Sync + 'static,
{
    pub fn new(
        env: Arc<Env>,
        id: Id,
        user_life_cycle_handle: LifeCycleHandle<Id, State, Action>,
        transition: Transition<Id, State, Action>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(8);
        tokio::spawn(run_entity(
            env,
            id,
            receiver,
            user_life_cycle_handle,
            transition,
        ));
        Self {
            id_type: PhantomData,
            state_type: PhantomData,
            sender,
        }
    }

    pub async fn act(&self, action: Action) {
        let _ = self.sender.send(action).await.expect("Send failed");
    }
}

pub async fn start_life_cycle<
    Id: Clone + Send + Sync + Ord + 'static,
    State: Clone + Send + Sync + Default + 'static,
    Action: Clone + Send + Sync + 'static,
>(
    env: Arc<Env>,
    life_cycle_handle: LifeCycleHandle<Id, State, Action>,
    mut receiver: Receiver<(Id, Action)>,
    transition: Transition<Id, State, Action>,
) -> ! {
    let mut handle_by_id = std::collections::BTreeMap::<Id, Handle<Id, State, Action>>::new();

    while let Some((id, action)) = receiver.recv().await {
        match handle_by_id.contains_key(&id) {
            true => (),
            false => {
                let handle = Handle::new(
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
