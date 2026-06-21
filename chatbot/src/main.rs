#![feature(never_type)]

mod configuration;
mod externals;
mod model_pack;
mod roles;
mod services;
mod state_machines;
mod tools;
mod types;

use serenity::all::{Http, HttpBuilder};
use services::discord::*;
use services::llama_cpp::LlamaCppService;
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::state_machines::conversation_state_machine::init_conversation_state_machine;

#[allow(dead_code)]
#[derive(Clone)]
pub struct Env {
    discord_http: Arc<Http>,
    llama_cpp: Arc<LlamaCppService>,
    announce_tool_use: bool,
}

async fn init_env() -> anyhow::Result<Env> {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    let llama_cpp_service = LlamaCppService::new().await?;

    let discord_http = Arc::new(HttpBuilder::new(discord_token).build());

    Ok(Env {
        discord_http,
        llama_cpp: Arc::new(llama_cpp_service),
        announce_tool_use: configuration::features::ANNOUNCE_TOOL_USE,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<!> {
    let env = init_env().await?;
    init_conversation_state_machine(env);

    // Pre-build the bash sandbox image in the background so the first run_bash_command isn't slow;
    // boot doesn't wait on it, and spawning a worker rebuilds it if this hasn't finished/failed.
    tokio::spawn(async {
        match externals::bash_container_external::ensure_worker_image().await {
            Ok(()) => println!("[startup] bash sandbox image ready"),
            Err(e) => eprintln!("[startup] bash sandbox image prebuild failed: {e} (will retry on first use)"),
        }
    });

    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    let mut set = JoinSet::new();

    let clients = vec![run_discord(prepare_discord_client(discord_token).await?)];

    clients.into_iter().for_each(|client| {
        set.spawn(client);
    });

    let _ = set.join_next().await;

    panic!("spawned handlers closed")
}
