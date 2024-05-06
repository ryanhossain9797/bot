use std::{future::Future, pin::Pin, sync::Arc};

use serenity::all::CreateMessage;
use tokio::sync::mpsc::{self, Receiver};

use crate::{
    lib_life_cycle::{
        run_entity, ExternalOperation, LifeCycleHandle, Transition, TransitionResult,
    },
    models::user::{User, UserAction, UserChannel, UserHandle, UserId},
    Env,
};

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

impl LifeCycleHandle<UserId, UserAction> {
    pub fn new(env: Arc<Env>, transition: Transition<UserId, User, UserAction>) -> Self {
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
}

impl UserHandle {
    pub fn new(
        env: Arc<Env>,
        user_id: UserId,
        user_life_cycle_handle: LifeCycleHandle<UserId, UserAction>,
        transition: Transition<UserId, User, UserAction>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(8);
        tokio::spawn(run_entity(
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
    user_life_cycle_handle: LifeCycleHandle<UserId, UserAction>,
    mut receiver: Receiver<(UserId, UserAction)>,
    transition: Transition<UserId, User, UserAction>,
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
        Err(err) => UserAction::SendResult(Arc::new(Err(err))),
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
                        Ok(_) => UserAction::SendResult(Arc::new(Ok(()))),
                        Err(err) => UserAction::SendResult(Arc::new(Err(anyhow::anyhow!(err)))),
                    }
                }
                Err(err) => UserAction::SendResult(Arc::new(Err(anyhow::anyhow!(err)))),
            }
        }
    }
}
