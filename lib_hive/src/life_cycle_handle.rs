use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{start_life_cycle, LifeCycleItem, PersistedLifeCycleItem, Schedule, Transition};

#[derive(Clone)]
pub struct LifeCycleHandle<Id, Action>
where
    Id: PersistedLifeCycleItem,
    Action: PersistedLifeCycleItem,
{
    pub sender: mpsc::Sender<(Id, Action)>,
}

impl<Id, Action> LifeCycleHandle<Id, Action>
where
    Id: PersistedLifeCycleItem + Ord + 'static,
    Action: PersistedLifeCycleItem + 'static,
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
    Id: PersistedLifeCycleItem + Ord + 'static,
    State: PersistedLifeCycleItem + Default + 'static,
    Action: PersistedLifeCycleItem + std::fmt::Debug + 'static,
    Env: LifeCycleItem + 'static,
>(
    env: Arc<Env>,
    transition: Transition<Id, State, Action, Env>,
    schedule: Schedule<State, Action>,
) -> LifeCycleHandle<Id, Action> {
    let (sender, receiver) = mpsc::channel(8);
    let user_life_cycle_handle = LifeCycleHandle { sender };
    tokio::spawn(start_life_cycle(
        env,
        user_life_cycle_handle.clone(),
        receiver,
        transition,
        schedule,
    ));
    user_life_cycle_handle
}
