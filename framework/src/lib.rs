mod bee_handle;
mod state_machine_handle;

use bee_handle::{new_entity, Handle};
use chrono::{DateTime, Utc};
pub use state_machine_handle::*;
use std::time::Duration;
use std::{future::Future, pin::Pin, sync::Arc};

use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;

pub type TransitionResult<Type, Action> =
    anyhow::Result<(Type, Vec<Pin<Box<dyn Future<Output = Action> + Send>>>)>;

pub type ExternalOperation<Action> = Pin<Box<dyn Future<Output = Action> + Send>>;

pub trait StateMachineItem: Send + Sync + Clone {}

impl<T: Send + Sync + Clone> StateMachineItem for T {}

pub trait PersistedStateMachineItem: StateMachineItem + serde::Serialize {}

impl<T> PersistedStateMachineItem for T where T: StateMachineItem + serde::Serialize {}

#[derive(Clone)]
pub struct Transition<Id, State, Action, Env>(
    pub  fn(
        Arc<Env>,
        Id,
        State,
        &Action,
    ) -> Pin<Box<dyn Future<Output = TransitionResult<State, Action>> + Send + '_>>,
);

#[derive(Clone)]
pub struct Construct<Id, State, Constructor>(pub fn(Id, Constructor) -> State);

#[derive(Clone)]
pub struct Scheduled<Action> {
    pub at: DateTime<Utc>,
    pub action: Action,
}

#[derive(Clone)]
pub struct Schedule<State, Action>(pub fn(&State) -> Vec<Scheduled<Action>>);

pub enum Activity<Action: PersistedStateMachineItem + 'static> {
    StateMachineAction(Action),
    ScheduledWakeup,
    DeleteSelf,
}

pub enum Input<Constructor, Action> {
    Construct(Constructor),
    Act(Action),
}

fn arm_schedule<State, Action>(
    schedule: &Schedule<State, Action>,
    state: &State,
    now: DateTime<Utc>,
    self_sender: &Sender<Activity<Action>>,
    maybe_scheduled: &mut Option<JoinHandle<()>>,
) where
    Action: PersistedStateMachineItem + 'static,
{
    if let Some(existing) = maybe_scheduled.take() {
        existing.abort();
    }

    let mut scheduled = schedule.0(state);
    scheduled.sort_by_key(|scheduled| scheduled.at);

    if let Some(scheduled) = scheduled.into_iter().next() {
        let at = scheduled.at;
        let self_sender = self_sender.clone();
        let timer_handle = tokio::spawn(async move {
            match (at - now).to_std() {
                Ok(sleep_duration) => {
                    tokio::time::sleep(sleep_duration).await;
                    while Utc::now() < at {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    let _ = self_sender.send(Activity::ScheduledWakeup).await;
                }
                Err(_negative_time_error) => {
                    let _ = self_sender.send(Activity::ScheduledWakeup).await;
                }
            }
        });

        *maybe_scheduled = Some(timer_handle);
    }
}

async fn run_entity<
    Id: PersistedStateMachineItem + Ord + 'static,
    State: PersistedStateMachineItem + 'static,
    Constructor: PersistedStateMachineItem + 'static,
    Action: PersistedStateMachineItem + std::fmt::Debug + 'static,
    Env: StateMachineItem + 'static,
>(
    env: Arc<Env>,
    id: Id,
    initial_state: State,
    mut receiver: Receiver<Activity<Action>>,
    handle: StateMachineHandle<Id, Constructor, Action>,
    transition: Transition<Id, State, Action, Env>,
    schedule: Schedule<State, Action>,
    self_sender: Sender<Activity<Action>>,
) {
    let mut state = initial_state;
    let mut maybe_scheduled: Option<JoinHandle<()>> = None;

    arm_schedule(&schedule, &state, Utc::now(), &self_sender, &mut maybe_scheduled);

    while let Some(activity) = receiver.recv().await {
        let now = Utc::now();

        let activity_str = match &activity {
            Activity::StateMachineAction(action) => format!("Action: {action:?}"),
            Activity::ScheduledWakeup => "ScheduledWakeup".to_string(),
            Activity::DeleteSelf => "DeleteSelf".to_string(),
        };
        println!(
            "TRANSITION AT {now} - StateMachine: {} - {}",
            std::any::type_name::<State>()
                .split("::")
                .last()
                .expect("Split should have at least one element"),
            activity_str
        );
        match activity {
            Activity::StateMachineAction(action) => {
                if let Ok((updated_state, external)) =
                    transition.0(env.clone(), id.clone(), state.clone(), &action).await
                {
                    arm_schedule(&schedule, &updated_state, now, &self_sender, &mut maybe_scheduled);

                    external.into_iter().for_each(|f| {
                        let handle: StateMachineHandle<Id, Constructor, Action> = handle.clone();
                        let id = id.clone();
                        tokio::spawn(async move {
                            let action = f.await;
                            handle.act(id, action).await;
                        });
                    });
                    state = updated_state;
                }
            }
            Activity::ScheduledWakeup => {
                let mut scheduled = schedule.0(&state);
                scheduled.sort_by_key(|scheduled| scheduled.at);

                let earliest = scheduled.into_iter().next();

                if let Some(scheduled) = earliest {
                    let sleep_for = scheduled.at - now;

                    match sleep_for.to_std() {
                        Ok(_time_left) => {
                            println!("Not Ready"); //TODO handle unexpected wakeup
                        }
                        Err(_negative_time_error) => {
                            let _ = self_sender
                                .send(Activity::StateMachineAction(scheduled.action))
                                .await;
                        }
                    }
                }
            }
            Activity::DeleteSelf => todo!(),
        }
    }
}

async fn start_state_machine<
    Id: PersistedStateMachineItem + Ord + 'static,
    State: PersistedStateMachineItem + 'static,
    Constructor: PersistedStateMachineItem + 'static,
    Action: PersistedStateMachineItem + std::fmt::Debug + 'static,
    Env: StateMachineItem + 'static,
>(
    env: Arc<Env>,
    state_machine_handle: StateMachineHandle<Id, Constructor, Action>,
    mut receiver: Receiver<(Id, Input<Constructor, Action>)>,
    construct: Construct<Id, State, Constructor>,
    transition: Transition<Id, State, Action, Env>,
    schedule: Schedule<State, Action>,
) -> ! {
    let mut handle_by_id = std::collections::BTreeMap::<Id, Handle<Action>>::new();

    while let Some((id, input)) = receiver.recv().await {
        match input {
            Input::Construct(constructor) => {
                if handle_by_id.contains_key(&id) {
                    continue;
                }
                let initial_state = construct.0(id.clone(), constructor);
                let handle = new_entity(
                    env.clone(),
                    id.clone(),
                    initial_state,
                    state_machine_handle.clone(),
                    transition.clone(),
                    schedule.clone(),
                );
                handle_by_id.insert(id, handle);
            }
            Input::Act(action) => match handle_by_id.get(&id) {
                Some(handle) => {
                    let handle = handle.clone();
                    tokio::spawn(async move { handle.act(action).await });
                }
                None => {
                    eprintln!(
                        "[warn] action {action:?} for unconstructed entity; dropping (Construct must precede Act)"
                    );
                }
            },
        }
    }
    panic!()
}
