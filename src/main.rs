use std::time::Duration;

use models::bot::{BotAction, BotHandle};

mod external_connections;
mod lifecycles;
mod models;

use external_connections::discord::*;

#[tokio::main]
async fn main() {
    let handle = BotHandle::new();
    let action = BotAction::Ping {
        message: "Ping".to_owned(),
    };

    let _ = handle.act(action).await;

    tokio::spawn(run_discord(handle.clone()));

    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
