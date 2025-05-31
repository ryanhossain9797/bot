#![feature(never_type)]

mod configuration;
mod external_connections;
mod life_cycles;
mod models;

use external_connections::discord::*;
use lib_hive::{new_life_cycle, Schedule, Transition};
use life_cycles::user_life_cycle::user_transition;
use models::bot::{BotAction, BotHandle};
use serenity::all::{Http, HttpBuilder};
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::life_cycles::user_life_cycle::schedule;

#[derive(Clone)]
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

    let discord_http =
        Arc::new(HttpBuilder::new(configuration::client_tokens::discord_token).build());
    let env = Arc::new(Env {
        discord_http,
        bot_singleton_handle,
    });

    let user_life_cycle = new_life_cycle(env, Transition(user_transition), Schedule(schedule));

    let discord_client =
        prepare_discord_client(configuration::client_tokens::discord_token, user_life_cycle)
            .await?;

    let mut set = JoinSet::new();

    let clients = vec![run_discord(discord_client)];

    clients.into_iter().for_each(|client| {
        set.spawn(client);
    });

    let _ = set.join_next().await;

    panic!("spawned handlers closed")
}
