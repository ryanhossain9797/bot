use serenity::all::{CreateMessage, Http};

use crate::{
    external_connections::common::get_client_token,
    models::user::{UserChannel, UserId},
};

pub async fn placeholder_handle_bot_message(
    user_id: UserId,
    msg: String,
    start_conversation: bool,
) -> anyhow::Result<()> {
    let user_id = match user_id.0 {
        UserChannel::Discord => serenity::all::UserId::new(user_id.1.parse::<u64>()?),
        _ => panic!("Telegram not yet implemented"),
    };
    let http = Http::new(
        get_client_token("discord_token")
            .ok_or_else(|| anyhow::anyhow!("Failed to load Discord Token"))?,
    );
    let dm_channel_result = match user_id.to_user(&http).await {
        Ok(user) => user.create_dm_channel(&http).await,
        Err(e) => Err(e),
    };

    match dm_channel_result {
        Ok(channel) => {
            let _ = channel
                .send_message(
                    &http,
                    CreateMessage::new().content(format!("You said {msg}")),
                )
                .await;
        }
        Err(_) => (),
    }

    Ok(())
}
