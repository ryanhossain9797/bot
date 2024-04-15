use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use serenity::{
    all::{CreateMessage, Http},
    model::user,
};
use tokio::sync::mpsc::{self, Receiver};

use crate::{
    lib_life_cycle::{ExternalOperation, TransitionResult},
    life_cycles::user_life_cycle,
    models::user::{User, UserAction, UserChannel, UserHandle, UserId},
    Env,
};

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

#[derive(Clone)]
pub struct Transition(
    pub  fn(
        Arc<Env>,
        UserId,
        User,
        UserAction,
    ) -> Pin<Box<dyn Future<Output = UserTransitionResult> + Send>>,
);
#[derive(Clone)]
pub struct UserLifeCycleHandle {
    pub sender: mpsc::Sender<(UserId, UserAction)>,
}

impl<'a> UserLifeCycleHandle {
    pub fn new(env: Arc<Env>, transition: Transition) -> Self {
        let (sender, receiver) = mpsc::channel(8);
        let user_life_cycle_handle = Self { sender };
        tokio::spawn(start_life_cycle(
            env,
            user_life_cycle_handle.clone(),
            receiver,
            transition,
        ));
        user_life_cycle_handle
    }

    pub async fn act(&self, user_id: UserId, user_action: UserAction) {
        let _ = self
            .sender
            .send((user_id, user_action))
            .await
            .expect("Send failed");
    }
}

impl UserHandle {
    pub fn new(
        env: Arc<Env>,
        user_id: UserId,
        user_life_cycle_handle: UserLifeCycleHandle,
        transition: Transition,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(8);
        tokio::spawn(run_user(
            env,
            user_id,
            receiver,
            user_life_cycle_handle,
            transition,
        ));
        Self { sender }
    }

    pub async fn act(&self, user_action: UserAction) {
        let _ = self.sender.send(user_action).await.expect("Send failed");
    }
}

async fn start_life_cycle(
    env: Arc<Env>,
    user_life_cycle_handle: UserLifeCycleHandle,
    mut receiver: Receiver<(UserId, UserAction)>,
    transition: Transition,
) -> ! {
    let mut handle_by_user = std::collections::BTreeMap::<UserId, UserHandle>::new();

    while let Some((user_id, action)) = receiver.recv().await {
        match handle_by_user.contains_key(&user_id) {
            true => (),
            false => {
                let handle = UserHandle::new(
                    env.clone(),
                    user_id.clone(),
                    user_life_cycle_handle.clone(),
                    transition.clone(),
                );
                handle_by_user.insert(user_id.clone(), handle.clone());
            }
        }
        let user_handle = handle_by_user[&user_id].clone();
        tokio::spawn(async move { user_handle.act(action).await });
    }
    panic!()
}

pub async fn run_user(
    env: Arc<Env>,
    user_id: UserId,
    mut receiver: Receiver<UserAction>,
    handle: UserLifeCycleHandle,
    transition: Transition,
) {
    let mut user = User { action_count: 0 };
    while let Some(action) = receiver.recv().await {
        match transition.0(env.clone(), user_id.clone(), user.clone(), action).await {
            Ok((updated_user, external)) => {
                user = updated_user;
                external.into_iter().for_each(|f| {
                    let handle = handle.clone();
                    let user_id = user_id.clone();
                    tokio::spawn(async move {
                        let action = f.await;
                        handle.act(user_id, action).await;
                    });
                });
            }
            Err(_) => (),
        }
    }
}

async fn user_transition(
    env: Arc<Env>,
    user_id: UserId,
    user: User,
    action: UserAction,
) -> UserTransitionResult {
    match action {
        UserAction::NewMessage {
            msg,
            start_conversation,
        } => {
            let mut external = Vec::<UserExternalOperation>::new();

            external.push(Box::pin(placeholder_handle_bot_message(
                env.clone(),
                user_id.clone(),
                msg,
            )));

            let user = User {
                action_count: user.action_count + 1,
            };

            println!("Id: {0} {1}", user_id.1, user.action_count);

            Ok((user, external))
        }
        UserAction::SendResult(send_result) => {
            println!("Send Succesful?: {0}", send_result.is_ok());
            Ok((user.clone(), Vec::new()))
        }
    }
}

pub fn user_transition_wrapper(
    env: Arc<Env>,
    user_id: UserId,
    user: User,
    action: UserAction,
) -> Pin<Box<dyn Future<Output = UserTransitionResult> + Send>> {
    let fut = user_transition(env, user_id, user, action);
    Box::pin(fut)
}

pub async fn placeholder_handle_bot_message(
    env: Arc<Env>,
    user_id: UserId,
    msg: String,
) -> UserAction {
    let user_id_result = match user_id.0 {
        UserChannel::Discord => {
            let user_id_result = user_id.1.parse::<u64>();
            match user_id_result {
                Ok(user_id) => Ok(serenity::all::UserId::new(user_id)),
                Err(err) => Err(anyhow::anyhow!(err)),
            }
        }
        _ => panic!("Telegram not yet implemented"),
    };
    match user_id_result {
        Err(err) => UserAction::SendResult(Err(err)),
        Ok(user_id) => {
            let dm_channel_result = match user_id.to_user(&env.discord_http).await {
                Ok(user) => user.create_dm_channel(&env.discord_http).await,
                Err(e) => Err(e),
            };

            match dm_channel_result {
                Ok(channel) => {
                    let res = channel
                        .send_message(
                            &env.discord_http,
                            CreateMessage::new().content(format!("You said {msg}")),
                        )
                        .await;
                    match res {
                        Ok(_) => UserAction::SendResult(Ok(())),
                        Err(err) => UserAction::SendResult(Err(anyhow::anyhow!(err))),
                    }
                }
                Err(err) => UserAction::SendResult(Err(anyhow::anyhow!(err))),
            }
        }
    }
}
