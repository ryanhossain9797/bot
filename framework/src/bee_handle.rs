use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    run_entity, Activity, LifeCycleHandle, LifeCycleItem, PersistedLifeCycleItem, Schedule,
    Transition,
};

#[derive(Clone)]
pub struct Handle<Action>
where
    Action: PersistedLifeCycleItem + 'static,
{
    pub sender: mpsc::Sender<Activity<Action>>,
}

impl<Action> Handle<Action>
where
    Action: PersistedLifeCycleItem + 'static,
{
    pub async fn act(&self, action: Action) {
        let _ = self
            .sender
            .send(Activity::LifeCycleAction(action))
            .await
            .expect("Send failed");
    }
}

pub fn new_entity<
    Id: PersistedLifeCycleItem + Ord + 'static,
    State: PersistedLifeCycleItem + 'static + Default,
    Action: PersistedLifeCycleItem + std::fmt::Debug + 'static,
    Env: LifeCycleItem + 'static,
>(
    env: Arc<Env>,
    id: Id,
    user_life_cycle_handle: LifeCycleHandle<Id, Action>,
    transition: Transition<Id, State, Action, Env>,
    schedule: Schedule<State, Action>,
) -> Handle<Action> {
    let (sender, receiver) = mpsc::channel(8);
    tokio::spawn(run_entity(
        env,
        id,
        receiver,
        user_life_cycle_handle,
        transition,
        schedule,
        sender.clone(),
    ));
    Handle { sender }
}
