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
// use services::llama_cpp::LlamaCppService; // Disconnected - will be replaced by Ollama
use services::ollama::OllamaService;
use std::sync::Arc;
use tokio::task::JoinSet;

#[derive(Clone)]
struct Env {
    discord_http: Arc<Http>,
    bot_singleton_handle: BotHandle,
    // llama_cpp: Arc<LlamaCppService>, // Disconnected - base image doesn't have GGUF
    ollama: Arc<OllamaService>,
}

static ENV: Lazy<Arc<Env>> = Lazy::new(|| {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;
    // Llama.cpp initialization disconnected - will be replaced by Ollama
    // let llama_cpp_service = LlamaCppService::new().expect("Failed to initialize Llama.cpp");
    
    let ollama_service = OllamaService::new().expect("Failed to initialize Ollama");

    Arc::new(Env {
        discord_http: Arc::new(HttpBuilder::new(discord_token).build()),
        bot_singleton_handle: BotHandle::new(),
        // llama_cpp: Arc::new(llama_cpp_service),
        ollama: Arc::new(ollama_service),
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
