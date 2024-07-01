use chrono::{DateTime, NaiveDateTime, Utc};
use std::time::Duration;
use std::{future::Future, pin::Pin, sync::Arc};

use tokio::sync::mpsc::{self, Receiver};
use tokio::task::JoinHandle;

pub type TransitionResult<Type, Action> =
    anyhow::Result<(Type, Vec<Pin<Box<dyn Future<Output = Action> + Send>>>)>;

pub type ExternalOperation<Action> = Pin<Box<dyn Future<Output = Action> + Send>>;

pub trait LifeCycleItem: Send + Sync + Clone {}

impl<T: Send + Sync + Clone> LifeCycleItem for T {}

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
pub struct Scheduled<Action> {
    at: DateTime<Utc>,
    action: Action,
}

#[derive(Clone)]
pub struct Schedule<State, Action>(pub fn(&State) -> Vec<Scheduled<Action>>);

pub enum Activity<Action: LifeCycleItem + 'static> {
    LifeCycleAction(Action),
    ScheduledWakeup,
    DeleteSelf,
}

#[derive(Clone)]
pub struct LifeCycleHandle<Id, Action>
where
    Id: LifeCycleItem,
    Action: LifeCycleItem,
{
    pub sender: mpsc::Sender<(Id, Action)>,
}

impl<Id, Action> LifeCycleHandle<Id, Action>
where
    Id: LifeCycleItem + Ord + 'static,
    Action: LifeCycleItem + 'static,
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
    Id: LifeCycleItem + Ord + 'static,
    State: LifeCycleItem + Default + 'static,
    Action: LifeCycleItem + 'static,
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

async fn run_entity<
    Id: LifeCycleItem + Ord + 'static,
    State: LifeCycleItem + Default + 'static,
    Action: LifeCycleItem + 'static,
    Env: LifeCycleItem + 'static,
>(
    env: Arc<Env>,
    id: Id,
    mut receiver: Receiver<Activity<Action>>,
    handle: LifeCycleHandle<Id, Action>,
    transition: Transition<Id, State, Action, Env>,
    schedule: Schedule<State, Action>,
) {
    let now = Utc::now();
    let mut state = State::default();
    let mut maybe_scheduled: Option<JoinHandle<()>> = None;

    while let Some(activity) = receiver.recv().await {
        match activity {
            Activity::LifeCycleAction(action) => {
                match transition.0(env.clone(), id.clone(), state.clone(), &action).await {
                    Ok((updated_user, external)) => {
                        match &maybe_scheduled {
                            Some(scheduled) => {
                                scheduled.abort();
                            }
                            None => {}
                        }
                        let mut scheduled = schedule.0(&updated_user);

                        scheduled.sort_by_key(|scheduled| scheduled.at);

                        let earliest = scheduled.first().map(|scheduled| scheduled.at);

                        match earliest {
                            Some(scheduled_at) => {
                                let handle = tokio::spawn(async move {
                                    let sleep_for = (scheduled_at - now).num_milliseconds();
                                    match sleep_for < 0 {
                                        true => {}
                                        false => {
                                            tokio::time::sleep(Duration::from_millis(
                                                sleep_for as u64,
                                            ))
                                            .await;
                                        }
                                    }

                                    //run timer here
                                });

                                maybe_scheduled = Some(handle)
                            }
                            None => {}
                        }

                        external.into_iter().for_each(|f| {
                            let handle: LifeCycleHandle<Id, Action> = handle.clone();
                            let user_id = id.clone();
                            tokio::spawn(async move {
                                let action = f.await;
                                handle.act(user_id, action).await;
                            });
                        });
                        state = updated_user;
                    }
                    Err(_) => (),
                }
            }
            Activity::ScheduledWakeup => {
                let mut scheduled = schedule.0(&state);
                scheduled.sort_by_key(|scheduled| scheduled.at);
            }
            Activity::DeleteSelf => todo!(),
        }
    }
}

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
    Action: LifeCycleItem + 'static,
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
    ));
    Handle { sender }
}

async fn start_life_cycle<
    Id: LifeCycleItem + Ord + 'static,
    State: LifeCycleItem + Default + 'static,
    Action: LifeCycleItem + 'static,
    Env: LifeCycleItem + 'static,
>(
    env: Arc<Env>,
    life_cycle_handle: LifeCycleHandle<Id, Action>,
    mut receiver: Receiver<(Id, Action)>,
    transition: Transition<Id, State, Action, Env>,
    schedule: Schedule<State, Action>,
) -> ! {
    let mut handle_by_id = std::collections::BTreeMap::<Id, Handle<Action>>::new();

    while let Some((id, action)) = receiver.recv().await {
        match handle_by_id.contains_key(&id) {
            true => (),
            false => {
                let handle = new_entity(
                    env.clone(),
                    id.clone(),
                    life_cycle_handle.clone(),
                    transition.clone(),
                    schedule.clone(),
                );
                handle_by_id.insert(id.clone(), handle.clone());
            }
        }
        let handle = handle_by_id[&id].clone();
        tokio::spawn(async move { handle.act(action).await });
    }
    panic!()
}
