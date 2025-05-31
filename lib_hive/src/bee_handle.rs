use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{run_entity, Activity, LifeCycleHandle, LifeCycleItem, Schedule, Transition};

#[derive(Clone)]
pub struct Handle<Action>
where
    Action: LifeCycleItem + 'static,
{
    pub sender: mpsc::Sender<Activity<Action>>,
}

impl<Action> Handle<Action>
where
    Action: LifeCycleItem + 'static,
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
    Id: LifeCycleItem + Ord + 'static,
    State: LifeCycleItem + 'static + Default,
    Action: LifeCycleItem + std::fmt::Debug + 'static,
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
