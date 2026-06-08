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
    pub async fn act(&self, id: Id, action: Action) {
        let _ = self
            .sender
            .send((id, action))
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
    let handle = StateMachineHandle { sender };
    tokio::spawn(start_state_machine(
        env,
        handle.clone(),
        receiver,
        transition,
        schedule,
    ));
    handle
}
