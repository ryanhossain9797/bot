use std::{future::Future, ops::Add, pin::Pin, sync::Arc, time::Duration};

use crate::{
    models::user::{User, UserAction, UserChannel, UserId, UserState},
    Env,
};
use chrono::{Duration as ChronoDuration, Utc};
use lib_hive::{ExternalOperation, Scheduled, TransitionResult};
use serenity::all::CreateMessage;

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

pub fn user_transition(
    env: Arc<Env>,
    user_id: UserId,
    user: User,
    action: &UserAction,
) -> Pin<Box<dyn Future<Output = UserTransitionResult> + Send + '_>> {
    Box::pin(async move {
        match (user.state, action) {
            (
                UserState::Idle,
                UserAction::NewMessage {
                    msg,
                    start_conversation,
                },
            ) => {
                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(placeholder_handle_bot_message(
                    env.clone(),
                    user_id.clone(),
                    msg.to_string(),
                )));

                let user = User {
                    state: UserState::RespondingToMessage,
                };

                println!("Id: {0} {1:?}", user_id.1, user.state);

                Ok((user, external))
            }
            (UserState::RespondingToMessage, UserAction::SendResult(send_result)) => Ok((
                User {
                    state: UserState::WaitingToSayGoodbye(Some(
                        Utc::now() + ChronoDuration::milliseconds(10_000),
                    )),
                    ..user
                },
                Vec::new(),
            )),
            (UserState::WaitingToSayGoodbye(_), UserAction::Poke) => {
                println!("Poked");

                let mut external = Vec::<UserExternalOperation>::new();

                external.push(Box::pin(placeholder_handle_bot_message(
                    env.clone(),
                    user_id.clone(),
                    "Goodbye".to_string(),
                )));

                Ok((
                    User {
                        state: UserState::SayingGoodbye,
                    },
                    external,
                ))
            }
            (UserState::SayingGoodbye, UserAction::SendResult(_)) => Ok((
                User {
                    state: UserState::Idle,
                },
                Vec::new(),
            )),
            _ => Err(anyhow::anyhow!("Invalid state or action")),
        }
    })
}

pub fn schedule(user: &User) -> Vec<Scheduled<UserAction>> {
    match user.state {
        UserState::WaitingToSayGoodbye(Some(poke_at)) => {
            vec![Scheduled {
                at: poke_at,
                action: UserAction::Poke,
            }]
        }
        _ => Vec::new(),
    }
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
