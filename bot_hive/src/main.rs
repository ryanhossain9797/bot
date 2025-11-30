#![feature(never_type)]

mod configuration;
mod connectors;
mod life_cycles;
mod models;
mod services;

use lib_hive::{new_life_cycle, Schedule, Transition};
use life_cycles::user_life_cycle::user_transition;
use models::bot::{BotAction, BotHandle};
use models::user::{User, UserId};
use once_cell::sync::Lazy;
use serenity::all::{Http, HttpBuilder};
use services::discord::*;
use services::llm::LlmService;
use std::sync::Arc;
use tokio::task::JoinSet;

#[derive(Clone)]
struct Env {
    discord_http: Arc<Http>,
    bot_singleton_handle: BotHandle,
    llm: Arc<LlmService>,
}

static ENV: Lazy<Arc<Env>> = Lazy::new(|| {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;
    let llm_service = LlmService::new().expect("Failed to initialize LLM");

    Arc::new(Env {
        discord_http: Arc::new(HttpBuilder::new(discord_token).build()),
        bot_singleton_handle: BotHandle::new(),
        llm: Arc::new(llm_service),
    })
});

#[tokio::main]
async fn main() -> anyhow::Result<!> {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    let mut set = JoinSet::new();

    let clients = vec![run_discord(prepare_discord_client(discord_token).await?)];

    clients.into_iter().for_each(|client| {
        set.spawn(client);
    });

    let _ = set.join_next().await;

    panic!("spawned handlers closed")
}
