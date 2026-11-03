#![feature(never_type)]

mod configuration;
mod externals;
mod llm;
mod prompts;
mod services;
mod state_machines;
mod tools;
mod types;

use serenity::all::{Http, HttpBuilder};
use services::discord::*;
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::llm::{LlmBackend, LlmBackendConfig, NativeLlm, RemoteLlm};
use crate::state_machines::conversation_state_machine::ConversationMachine;
use crate::state_machines::memory_manager_state_machine::MemoryManagerMachine;
use crate::state_machines::reminder_state_machine::ReminderForConversationMachine;

pub const BOT_NAME: &str = "A2";

#[derive(Clone)]
pub struct Env {
    discord_http: Arc<Http>,
    llm: Arc<LlmBackend>,
    announce_tool_use: bool,
}

async fn init_env() -> anyhow::Result<Env> {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    let llm = match configuration::llm::BACKEND {
        LlmBackendConfig::Native => LlmBackend::Native(NativeLlm::load().await?),
        LlmBackendConfig::Remote(config) => {
            println!(
                "[startup] remote LLM backend: {} (primary {}, utility {})",
                config.base_url,
                config.primary_model,
                config.utility_model.unwrap_or("none — compaction disabled")
            );
            LlmBackend::Remote(RemoteLlm::new(config)?)
        }
    };

    let discord_http = Arc::new(HttpBuilder::new(discord_token).build());

    Ok(Env {
        discord_http,
        llm: Arc::new(llm),
        announce_tool_use: configuration::features::ANNOUNCE_TOOL_USE,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<!> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("bench") {
        let (message, image) = (args.next(), args.next());
        tokio::task::spawn_blocking(move || NativeLlm::bench(message, image)).await??;
        std::process::exit(0);
    }

    re_framework::init_turso_store("framework_db/chatbot.db").await?;
    let env = init_env().await?;
    re_framework::register::<ConversationMachine>(env.clone());
    re_framework::register::<MemoryManagerMachine>(env.clone());
    re_framework::register::<ReminderForConversationMachine>(env);
    re_framework::start();

    tokio::spawn(async {
        match externals::bash_container_external::ensure_worker_image().await {
            Ok(()) => println!("[startup] bash sandbox image ready"),
            Err(e) => eprintln!(
                "[startup] bash sandbox image prebuild failed: {e} (will retry on first use)"
            ),
        }
    });

    tokio::spawn(async {
        match externals::searxng_external::ensure_searxng().await {
            Ok(()) => println!("[startup] searxng ready"),
            Err(e) => {
                eprintln!("[startup] searxng bring-up failed: {e} (will retry on first search)")
            }
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
