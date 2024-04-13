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

use super::user_lifecycle::placeholder_handle_bot_message;

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
        //Placeholder, There should be a user_lifecycle single actor, and that should recieve these instead
        BotAction::HandleMessage {
            user_id,
            start_conversation,
            msg,
        } => {
            let _ = placeholder_handle_bot_message(user_id, msg, start_conversation).await;
            Ok(())
        }
    }
}

pub async fn run_bot(mut bot: Bot) {
    while let Some(action) = bot.receiver.recv().await {
        bot_transition(&mut bot, action).await.unwrap();
    }
}
