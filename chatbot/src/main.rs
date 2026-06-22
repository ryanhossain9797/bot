#![feature(never_type)]

mod chat_format;
mod configuration;
mod externals;
mod model_pack;
mod roles;
mod services;
mod state_machines;
mod tools;
mod types;

use llama_cpp_2::{llama_backend::LlamaBackend, send_logs_to_tracing, LogOptions};
use serenity::all::{Http, HttpBuilder};
use services::discord::*;
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::roles::PrimaryRole;
use crate::state_machines::conversation_state_machine::init_conversation_state_machine;

#[derive(Clone)]
pub struct Env {
    discord_http: Arc<Http>,
    /// The primary conversational role, owning its loaded model (and a handle to the shared
    /// backend). Future roles (e.g. a memory compactor on its own model) become additional fields
    /// here; local ones share the one backend via its `Arc`.
    primary: Arc<PrimaryRole>,
    announce_tool_use: bool,
}

async fn init_env() -> anyhow::Result<Env> {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
    let backend = Arc::new(LlamaBackend::init()?);
    let primary =
        Arc::new(tokio::task::spawn_blocking(move || PrimaryRole::load(backend)).await??);

    let discord_http = Arc::new(HttpBuilder::new(discord_token).build());

    Ok(Env {
        discord_http,
        primary,
        announce_tool_use: configuration::features::ANNOUNCE_TOOL_USE,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<!> {
    let env = init_env().await?;
    init_conversation_state_machine(env);

    tokio::spawn(async {
        match externals::bash_container_external::ensure_worker_image().await {
            Ok(()) => println!("[startup] bash sandbox image ready"),
            Err(e) => eprintln!(
                "[startup] bash sandbox image prebuild failed: {e} (will retry on first use)"
            ),
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
