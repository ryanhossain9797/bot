#![feature(never_type)]

mod configuration;
mod externals;
mod models;
mod services;
mod state_machines;

use framework::{new_state_machine, Schedule, Transition};
use models::bot::{BotAction, BotHandle};
use models::user::{User, UserId};
use once_cell::sync::OnceCell;
use serenity::all::{Http, HttpBuilder};
use services::discord::*;
use services::llama_cpp::LlamaCppService;
use state_machines::user_state_machine::user_transition;
// use services::ollama::OllamaService;
use std::sync::Arc;
use tokio::task::JoinSet;

#[derive(Clone)]
struct Env {
    discord_http: Arc<Http>,
    bot_singleton_handle: BotHandle,
    llama_cpp: Arc<LlamaCppService>, // Disconnected - base image doesn't have GGUF
                                     // ollama: Arc<OllamaService>,
}

// ENV needs to be initialized asynchronously, so we use OnceCell
static ENV: OnceCell<Arc<Env>> = OnceCell::new();

async fn init_env() -> anyhow::Result<Arc<Env>> {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;
    // Llama.cpp initialization disconnected - will be replaced by Ollama
    let llama_cpp_service = LlamaCppService::new().expect("Failed to initialize Llama.cpp");

    // let ollama_service = OllamaService::new().await?;

    Ok(Arc::new(Env {
        discord_http: Arc::new(HttpBuilder::new(discord_token).build()),
        bot_singleton_handle: BotHandle::new(),
        llama_cpp: Arc::new(llama_cpp_service),
        // ollama: Arc::new(ollama_service),
    }))
}

#[tokio::main]
async fn main() -> anyhow::Result<!> {
    // Initialize ENV asynchronously
    let env = init_env().await?;
    if ENV.set(env.clone()).is_err() {
        panic!("ENV should only be initialized once");
    }

    let discord_token = configuration::client_tokens::DISCORD_TOKEN;

    let mut set = JoinSet::new();

    let clients = vec![run_discord(prepare_discord_client(discord_token).await?)];

    clients.into_iter().for_each(|client| {
        set.spawn(client);
    });

    let _ = set.join_next().await;

    panic!("spawned handlers closed")
}
