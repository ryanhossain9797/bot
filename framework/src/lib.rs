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

/// Extension of StateMachineItem that supports persistence via JSON serialization
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

/// Builds an entity's initial `State` from its `Id` and a domain-supplied `Constructor` payload.
/// This is the *only* thing that ever produces a starting state — there is no implicit `Default`
/// fallback — so the domain decides up front what a fresh entity looks like (e.g. carrying
/// construction-time facts that are then persisted on the state for the entity's whole life).
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

/// What the router receives for a given `Id`. `Construct` is the only path that creates an entity
/// (idempotent — a second `Construct` for an already-live id is a no-op); `Act` delivers an action
/// to an entity that must already exist. An `Act` for an unknown id is warned about and dropped —
/// there is no implicit/lazy creation, so the domain is responsible for constructing before acting.
pub enum Input<Constructor, Action> {
    Construct(Constructor),
    Act(Action),
}

/// (Re)arm the single pending timer for an entity from its current `state`: abort whatever timer
/// was running, ask `schedule` for the soonest due action, and spawn a task that fires a
/// `ScheduledWakeup` back into the entity at that time (immediately if it's already past). A state
/// whose schedule is empty leaves the entity with no timer. Called both on the constructed initial
/// state and after every transition, so a freshly-constructed (or, later, rehydrated) entity arms
/// its timers immediately rather than only after its first action.
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
                // Negative duration means the scheduled time has already passed — fire now.
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

    // Arm any timers implied by the constructed initial state up front, so correctness doesn't
    // depend on the initial state happening to have an empty schedule (it does today — Idle{None} —
    // but a rehydrated entity, #106, can start mid-flight and must time out without a first action).
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
                            // Negative duration means the scheduled time has already passed
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
            // The single, idempotent creation path. The router owns the existence check, so the
            // domain can construct unconditionally (e.g. on every inbound message) without tracking
            // a seen-set; a Construct for a live id is dropped. mpsc FIFO makes this race-free, and
            // an Act enqueued right after its Construct can never overtake it.
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
            // No implicit creation: an action for an id we've never constructed is a bug in the
            // caller (it should have constructed first), so warn and drop rather than silently
            // spinning up a default entity.
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
