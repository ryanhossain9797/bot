use std::sync::Arc;

use tokio::sync::mpsc::{self, Receiver};

use crate::Env;

#[derive(Clone)]
pub struct LifeCycleHandle<Id, Action> {
    pub sender: mpsc::Sender<(Id, Action)>,
}

impl<Id, Action> LifeCycleHandle<Id, Action> {
    pub fn new(env: Arc<Env>) -> Self {
        let (sender, receiver) = mpsc::channel(8);
        tokio::spawn(run_action(env, receiver));

        Self { sender }
    }

    pub async fn act(&self, id: Id, action: Action) {
        let _ = self.sender.send((id, action)).await.expect("Send failed");
    }
}

#[derive(Clone)]
pub struct Handle<Id, Action>
where
    Id: Clone,
{
    pub sender: mpsc::Sender<Action>,
}

impl<Id, Action, Data> Handle<Id, Action>
where
    Id: Ord + Clone + Send + Sync,
    Action: Clone + Send,
    Data: Sync + Send + Clone + Default,
{
    pub fn new(env: Arc<Env>, id: Id) -> Self {
        let (sender, receiver) = mpsc::channel(8);
        tokio::spawn(async move { run_user(env, id, Data::default(), receiver).await });

        Self { sender }
    }

    pub async fn act(&self, user_action: Action) {
        let _ = self.sender.send(user_action).await.expect("Send failed");
    }
}

async fn run_action<Id, Action>(env: Arc<Env>, mut receiver: Receiver<(Id, Action)>) -> !
where
    Id: Ord + Clone + Send + Sync,
    Action: Clone + Send,
{
    let mut handle_by_user = std::collections::BTreeMap::<Id, Handle<Id, Action>>::new();

    while let Some((user_id, action)) = receiver.recv().await {
        match handle_by_user.contains_key(&user_id) {
            true => (),
            false => {
                let handle = Handle::new(env.clone(), user_id.clone());
                handle_by_user.insert(user_id.clone(), handle.clone());
            }
        }
        let handle = handle_by_user[&user_id].clone();
        tokio::spawn(async move { handle.act(action).await });
    }
    panic!()
}

pub async fn run_user<Id, Action, Data>(
    env: Arc<Env>,
    id: Id,
    mut data: Data,
    mut receiver: Receiver<Action>,
) where
    Data: Clone + Default,
{
    while let Some(action) = receiver.recv().await {
        match transition(env.clone(), &id, &data, action).await {
            Ok(updated_user) => data = updated_user,
            Err(_) => (),
        }
    }
}

async fn transition<Id, Action, Data>(
    env: Arc<Env>,
    user_id: &Id,
    data: &Data,
    action: Action,
) -> anyhow::Result<Data>
where
    Data: Clone,
{
    Ok(data.clone())
}
