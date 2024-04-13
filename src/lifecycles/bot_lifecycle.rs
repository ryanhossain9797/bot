use serenity::{
    all::{CreateMessage, Http},
    futures::TryFutureExt,
    http,
    model::channel,
};
use tokio::{
    io,
    sync::{mpsc, oneshot},
};

use crate::{
    external_connections::common::get_client_token,
    models::bot::{Bot, BotAction, BotHandle},
};

impl Bot {
    pub fn new(receiver: mpsc::Receiver<BotAction>) -> Self {
        Self { receiver }
    }
}

impl BotHandle {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel(8);
        let bot = Bot::new(receiver);
        tokio::spawn(run_bot(bot));

        Self { sender }
    }

    pub async fn act(&self, action: BotAction) {
        let _ = self.sender.send(action).await.expect("Send failed");
    }
}

async fn bot_transition(bot: &mut Bot, action: BotAction) -> anyhow::Result<()> {
    match action {
        BotAction::Ping { message } => {
            let response = format!("Pong: {message}");
            println!("{response}");
            Ok(())
        }
        BotAction::HandleMessage {
            user_id,
            start_conversation,
            msg,
        } => {
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
    }
}

pub async fn run_bot(mut bot: Bot) {
    while let Some(action) = bot.receiver.recv().await {
        bot_transition(&mut bot, action).await.unwrap();
    }
}
