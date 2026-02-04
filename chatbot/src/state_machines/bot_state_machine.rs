use tokio::sync::mpsc;

use crate::models::bot::{Bot, BotAction, BotHandle};

impl Bot {
    pub fn new(receiver: mpsc::Receiver<BotAction>) -> Self {
        Self { receiver }
    }
}

#[allow(dead_code)]
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

async fn bot_transition(_bot: &mut Bot, action: BotAction) -> anyhow::Result<()> {
    match action {
        BotAction::Ping { message } => {
            let response = format!("Pong: {message}");
            println!("{response}");
            Ok(())
        }
    }
}

pub async fn run_bot(mut bot: Bot) {
    while let Some(action) = bot.receiver.recv().await {
        bot_transition(&mut bot, action).await.unwrap();
    }
}
