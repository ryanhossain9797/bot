use std::sync::Arc;

use serenity::all::CreateMessage;

use crate::{
    models::user::{UserAction, UserChannel, UserId},
    Env,
};

pub async fn handle_bot_message(env: Arc<Env>, user_id: UserId, msg: String) -> UserAction {
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
                        .send_message(&env.discord_http, CreateMessage::new().content(msg))
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
