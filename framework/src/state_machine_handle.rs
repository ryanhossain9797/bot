use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    start_state_machine, PersistedStateMachineItem, Schedule, StateMachineItem, Transition,
};

#[derive(Clone)]
pub struct StateMachineHandle<Id, Action>
where
    Id: PersistedStateMachineItem,
    Action: PersistedStateMachineItem,
{
    pub sender: mpsc::Sender<(Id, Action)>,
}

impl<Id, Action> StateMachineHandle<Id, Action>
where
    Id: PersistedStateMachineItem + Ord + 'static,
    Action: PersistedStateMachineItem + 'static,
{
    pub async fn act(&self, user_id: Id, user_action: Action) {
        let _ = self
            .sender
            .send((user_id, user_action))
            .await
            .expect("Send failed");
    }
}

pub fn new_state_machine<
    Id: PersistedStateMachineItem + Ord + 'static,
    State: PersistedStateMachineItem + Default + 'static,
    Action: PersistedStateMachineItem + std::fmt::Debug + 'static,
    Env: StateMachineItem + 'static,
>(
    env: Arc<Env>,
    transition: Transition<Id, State, Action, Env>,
    schedule: Schedule<State, Action>,
) -> StateMachineHandle<Id, Action> {
    let (sender, receiver) = mpsc::channel(8);
    let user_state_machine_handle = StateMachineHandle { sender };
    tokio::spawn(start_state_machine(
        env,
        user_state_machine_handle.clone(),
        receiver,
        transition,
        schedule,
    ));
    user_state_machine_handle
}
