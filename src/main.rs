use std::{future::IntoFuture, time::Duration};

use models::bot::{BotAction, BotHandle};

mod lifecycles;
mod models;

#[tokio::main]
async fn main() {
    let handle = BotHandle::new();
    let action = BotAction::Ping {
        message: "Hello".to_owned(),
    };

    let _ = handle.act(action).await;

    let _ = handle.on_kill.await;
}
