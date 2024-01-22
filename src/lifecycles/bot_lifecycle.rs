use tokio::{
    io,
    sync::{mpsc, oneshot},
};

use crate::models::bot::{Bot, BotAction, BotHandle};

impl Bot {
    pub fn new(receiver: mpsc::Receiver<BotAction>) -> Self {
        Self { receiver }
    }
}

impl BotHandle {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel(8);
        let bot = Bot::new(receiver);

        let (kill_send, kill_recv) = oneshot::channel();
        tokio::spawn(run_bot(bot, kill_send));

        Self {
            sender,
            on_kill: kill_recv,
        }
    }

    pub async fn act(&self, action: BotAction) {
        let _ = self.sender.send(action).await.expect("Send failed");
    }
}

async fn bot_transition(bot: &mut Bot, action: BotAction) -> io::Result<()> {
    match action {
        BotAction::Ping { message } => {
            let response = format!("Pong: {message}");
            println!("{response}");
            Ok(())
        }
    }
}

pub async fn run_bot(mut bot: Bot, kill_msg: oneshot::Sender<()>) {
    while let Some(action) = bot.receiver.recv().await {
        bot_transition(&mut bot, action).await.unwrap();
    }

    kill_msg.send(());
}
