#![feature(never_type)]

mod configuration;
mod connectors;
mod external_connections;
mod life_cycles;
mod models;

use external_connections::discord::*;
use lib_hive::{new_life_cycle, Schedule, Transition};
use life_cycles::user_life_cycle::user_transition;
use llama_cpp_2::{llama_backend::LlamaBackend, model::LlamaModel};
use models::bot::{BotAction, BotHandle};
use serenity::all::{Http, HttpBuilder};
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::{external_connections::llm::prepare_llm, life_cycles::user_life_cycle::schedule};

#[derive(Clone)]
struct Env {
    discord_http: Arc<Http>,
    bot_singleton_handle: BotHandle,
    llm: Arc<(LlamaModel, LlamaBackend)>,
}

#[tokio::main]
async fn main() -> anyhow::Result<!> {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    let bot_singleton_handle = BotHandle::new();
    let discord_http = Arc::new(HttpBuilder::new(discord_token).build());
    let llm = Arc::new(prepare_llm()?);

    let env = Arc::new(Env {
        discord_http,
        bot_singleton_handle,
        llm,
    });

    let user_life_cycle = new_life_cycle(env, Transition(user_transition), Schedule(schedule));

    let mut set = JoinSet::new();

    let clients = vec![run_discord(
        prepare_discord_client(discord_token, user_life_cycle).await?,
    )];

    clients.into_iter().for_each(|client| {
        set.spawn(client);
    });

    let _ = set.join_next().await;

    panic!("spawned handlers closed")
}
