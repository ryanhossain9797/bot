use std::sync::Arc;

use crate::models::user::UserAction;
use crate::models::user::UserChannel;
use crate::{models::user::UserId, Env};
use serenity::all::CreateMessage;

pub async fn send_message(env: Arc<Env>, user_id: UserId, message: String) -> UserAction {
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
        Err(err) => UserAction::MessageSent(Arc::new(Err(err))),
        Ok(user_id) => {
            let dm_channel_result = match user_id.to_user(&env.discord_http).await {
                Ok(user) => user.create_dm_channel(&env.discord_http).await,
                Err(e) => Err(e),
            };

            match dm_channel_result {
                Ok(channel) => {
                    let res = channel
                        .send_message(&env.discord_http, CreateMessage::new().content(&message))
                        .await;

                    match res {
                        Ok(_) => UserAction::MessageSent(Arc::new(Ok(()))),
                        Err(err) => UserAction::MessageSent(Arc::new(Err(anyhow::anyhow!(err)))),
                    }
                }
                Err(err) => UserAction::MessageSent(Arc::new(Err(anyhow::anyhow!(err)))),
            }
        }
    }
}
