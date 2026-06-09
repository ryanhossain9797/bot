use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    start_state_machine, Construct, Input, PersistedStateMachineItem, Schedule, StateMachineItem,
    Transition,
};

#[derive(Clone)]
pub struct StateMachineHandle<Id, Constructor, Action>
where
    Id: PersistedStateMachineItem,
    Constructor: PersistedStateMachineItem,
    Action: PersistedStateMachineItem,
{
    pub sender: mpsc::Sender<(Id, Input<Constructor, Action>)>,
}

impl<Id, Constructor, Action> StateMachineHandle<Id, Constructor, Action>
where
    Id: PersistedStateMachineItem + Ord + 'static,
    Constructor: PersistedStateMachineItem + 'static,
    Action: PersistedStateMachineItem + 'static,
{
    /// Ensure the entity for `id` exists, building its initial state from `constructor` *only if it
    /// isn't already live* — hence "maybe". Idempotent: safe to call before every action (the router
    /// drops it when the entity already exists). Always pair an `act` with a preceding
    /// `maybe_construct` for the same id.
    pub async fn maybe_construct(&self, id: Id, constructor: Constructor) {
        self.sender
            .send((id, Input::Construct(constructor)))
            .await
            .expect("Send failed");
    }

    /// Deliver an action to an already-constructed entity. If `id` was never constructed the router
    /// warns and drops it — there is no implicit creation.
    pub async fn act(&self, id: Id, action: Action) {
        self.sender
            .send((id, Input::Act(action)))
            .await
            .expect("Send failed");
    }
}

pub fn new_state_machine<
    Id: PersistedStateMachineItem + Ord + 'static,
    State: PersistedStateMachineItem + 'static,
    Constructor: PersistedStateMachineItem + 'static,
    Action: PersistedStateMachineItem + std::fmt::Debug + 'static,
    Env: StateMachineItem + 'static,
>(
    env: Arc<Env>,
    construct: Construct<Id, State, Constructor>,
    transition: Transition<Id, State, Action, Env>,
    schedule: Schedule<State, Action>,
) -> StateMachineHandle<Id, Constructor, Action> {
    let (sender, receiver) = mpsc::channel(8);
    let handle = StateMachineHandle { sender };
    tokio::spawn(start_state_machine(
        env,
        handle.clone(),
        receiver,
        construct,
        transition,
        schedule,
    ));
    handle
}
