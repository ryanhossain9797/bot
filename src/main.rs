#![feature(never_type)]
use std::{sync::Arc, time::Duration};

use life_cycles::user_life_cycle::UserLifeCycleHandle;
use models::bot::{BotAction, BotHandle};

mod external_connections;
mod life_cycles;
mod models;

use external_connections::{common::get_client_token, discord::*};
use serenity::all::{Http, HttpBuilder};

struct Env {
    discord_http: Arc<Http>,
    bot_singleton_handle: BotHandle,
}

#[tokio::main]
async fn main() -> anyhow::Result<!> {
    let bot_singleton_handle = BotHandle::new();
    let action = BotAction::Ping {
        message: "Ping".to_owned(),
    };

    let _ = bot_singleton_handle.act(action).await;
    let discord_token = get_client_token("discord_token")
        .ok_or_else(|| anyhow::anyhow!("Failed to load Discord Token"))?;

    let discord_http = Arc::new(HttpBuilder::new(&discord_token).build());
    let env = Arc::new(Env {
        discord_http,
        bot_singleton_handle,
    });
    let user_life_cycle = UserLifeCycleHandle::new(env);
    let discord_client = prepare_discord_client(discord_token, user_life_cycle).await?;

    let x = tokio::spawn(run_discord(discord_client));

    let _ = x.await;

    panic!("spawned handlers closed")
}
