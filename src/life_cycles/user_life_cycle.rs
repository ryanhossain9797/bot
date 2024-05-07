use std::{future::Future, pin::Pin, sync::Arc};

use serenity::all::CreateMessage;

use crate::{
    lib_life_cycle::{ExternalOperation, TransitionResult},
    models::user::{User, UserAction, UserChannel, UserId},
    Env,
};

type UserTransitionResult = TransitionResult<User, UserAction>;
type UserExternalOperation = ExternalOperation<UserAction>;

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
