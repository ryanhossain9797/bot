use std::{future::Future, ops::Add, pin::Pin, sync::Arc, time::Duration};

use crate::{
    models::user::{User, UserAction, UserChannel, UserId},
    Env,
};
use chrono::Utc;
use lib_hive::{ExternalOperation, Scheduled, TransitionResult};
use serenity::all::CreateMessage;

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

async fn user_transition(
    env: Arc<Env>,
    user_id: UserId,
    user: User,
    action: &UserAction,
) -> UserTransitionResult {
    match action {
        UserAction::Poke => {
            println!("Poked");
            Ok((
                User {
                    maybe_poke_at: None,
                    ..user
                },
                Vec::new(),
            ))
        }
        UserAction::NewMessage {
            msg,
            start_conversation,
        } => {
            let mut external = Vec::<UserExternalOperation>::new();

            external.push(Box::pin(placeholder_handle_bot_message(
                env.clone(),
                user_id.clone(),
                msg.to_string(),
            )));

            let user = User {
                action_count: user.action_count + 1,
                maybe_poke_at: Some(Utc::now().add(Duration::from_millis(2_000))), //replace with managed time,
            };

            println!("Id: {0} {1}", user_id.1, user.action_count);

            Ok((user, external))
        }
        UserAction::SendResult(send_result) => {
            println!("Send Succesful?: {0}", send_result.is_ok());
            Ok((
                User {
                    maybe_poke_at: Some(Utc::now().add(Duration::from_millis(2_000))), //replace with managed time
                    ..user
                },
                Vec::new(),
            ))
        }
    }
}

pub fn user_transition_wrapper(
    env: Arc<Env>,
    user_id: UserId,
    user: User,
    action: &UserAction,
) -> Pin<Box<dyn Future<Output = UserTransitionResult> + Send + '_>> {
    let fut = user_transition(env, user_id, user, action);
    Box::pin(fut)
}

pub fn schedule(user: &User) -> Vec<Scheduled<UserAction>> {
    match user.maybe_poke_at {
        Some(poke_at) => {
            vec![Scheduled {
                at: poke_at,
                action: UserAction::Poke,
            }]
        }
        None => Vec::new(),
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
