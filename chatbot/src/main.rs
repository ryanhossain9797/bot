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

use crate::roles::{PrimaryRole, UtilityRole};
use crate::state_machines::conversation_state_machine::ConversationMachine;
use crate::state_machines::memory_manager_state_machine::MemoryManagerMachine;

#[derive(Clone)]
pub struct Env {
    discord_http: Arc<Http>,
    primary: Arc<PrimaryRole>,
    utility: Option<Arc<UtilityRole>>,
    announce_tool_use: bool,
}

async fn init_env() -> anyhow::Result<Env> {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
    let backend = Arc::new(LlamaBackend::init()?);
    let primary = Arc::new(
        tokio::task::spawn_blocking({
            let backend = Arc::clone(&backend);
            move || PrimaryRole::load(backend)
        })
        .await??,
    );

    let utility = match tokio::task::spawn_blocking(move || UtilityRole::load(backend)).await? {
        Ok(role) => {
            println!("[startup] utility model ready");
            Some(Arc::new(role))
        }
        Err(e) => {
            eprintln!("[startup] utility model not loaded: {e:#} — compaction disabled");
            None
        }
    };

    let discord_http = Arc::new(HttpBuilder::new(discord_token).build());

    Ok(Env {
        discord_http,
        primary,
        utility,
        announce_tool_use: configuration::features::ANNOUNCE_TOOL_USE,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<!> {
    re_framework::init_turso_store("framework_db/chatbot.db").await?;
    let env = init_env().await?;
    re_framework::register::<ConversationMachine>(env.clone());
    re_framework::register::<MemoryManagerMachine>(env);
    re_framework::start();

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
