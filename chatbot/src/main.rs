#![feature(never_type)]

mod agents;
mod configuration;
mod externals;
mod types;
mod services;
mod state_machines;
mod tools;

use serenity::all::{Http, HttpBuilder};
use services::discord::*;
use services::llama_cpp::LlamaCppService;
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::services::lance_db::LanceService;
use crate::state_machines::conversation_state_machine::init_conversation_state_machine;

#[allow(dead_code)]
#[derive(Clone)]
pub struct Env {
    discord_http: Arc<Http>,
    lance_service: Arc<LanceService>,
    llama_cpp: Arc<LlamaCppService>,
    announce_tool_use: bool,
}

async fn init_env() -> anyhow::Result<Env> {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    let llama_cpp_service = LlamaCppService::new().await?;
    let lance_service = LanceService::new().await;

    let discord_http = Arc::new(HttpBuilder::new(discord_token).build());

    Ok(Env {
        discord_http,
        lance_service: Arc::new(lance_service),
        llama_cpp: Arc::new(llama_cpp_service),
        announce_tool_use: configuration::features::ANNOUNCE_TOOL_USE,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<!> {
    let env = init_env().await?;
    init_conversation_state_machine(env);

    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    let mut set = JoinSet::new();

    let clients = vec![run_discord(prepare_discord_client(discord_token).await?)];

    clients.into_iter().for_each(|client| {
        set.spawn(client);
    });

    let _ = set.join_next().await;

    panic!("spawned handlers closed")
}
