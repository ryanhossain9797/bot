use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    run_entity, Activity, PersistedStateMachineItem, Schedule, StateMachineHandle,
    StateMachineItem, Transition,
};

#[derive(Clone)]
pub struct Handle<Action>
where
    Action: PersistedStateMachineItem + 'static,
{
    pub sender: mpsc::Sender<Activity<Action>>,
}

impl<Action> Handle<Action>
where
    Action: PersistedStateMachineItem + 'static,
{
    pub async fn act(&self, action: Action) {
        let _ = self
            .sender
            .send(Activity::StateMachineAction(action))
            .await
            .expect("Send failed");
    }
}

pub fn new_entity<
    Id: PersistedStateMachineItem + Ord + 'static,
    State: PersistedStateMachineItem + 'static + Default,
    Action: PersistedStateMachineItem + std::fmt::Debug + 'static,
    Env: StateMachineItem + 'static,
>(
    env: Arc<Env>,
    id: Id,
    user_state_machine_handle: StateMachineHandle<Id, Action>,
    transition: Transition<Id, State, Action, Env>,
    schedule: Schedule<State, Action>,
) -> Handle<Action> {
    let (sender, receiver) = mpsc::channel(8);
    tokio::spawn(run_entity(
        env,
        id,
        receiver,
        user_state_machine_handle,
        transition,
        schedule,
        sender.clone(),
    ));
    Handle { sender }
}
