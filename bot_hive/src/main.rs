#![feature(never_type)]

mod configuration;
mod connectors;
mod life_cycles;
mod models;
mod services;

use lib_hive::{new_life_cycle, Schedule, Transition};
use life_cycles::user_life_cycle::user_transition;
use llama_cpp_2::{llama_backend::LlamaBackend, model::LlamaModel};
use models::bot::{BotAction, BotHandle};
use models::user::{User, UserId};
use once_cell::sync::Lazy;
use serenity::all::{Http, HttpBuilder};
use services::discord::*;
use std::sync::Arc;
use tokio::task::JoinSet;

use crate::services::llm::{prepare_llm, BasePrompt};

#[derive(Clone)]
struct Env {
    discord_http: Arc<Http>,
    bot_singleton_handle: BotHandle,
    llm: Arc<(LlamaModel, LlamaBackend)>,
    base_prompt: Arc<BasePrompt>,
}

static ENV: Lazy<Arc<Env>> = Lazy::new(|| {
    let discord_token = configuration::client_tokens::DISCORD_TOKEN;
    let (model, backend) = prepare_llm().expect("Failed to initialize LLM");
    let base_prompt = BasePrompt::new();

    if let Err(e) = crate::services::llm::create_session_file(
        &model,
        &backend,
        base_prompt.as_str(),
        base_prompt.session_path(),
    ) {
        eprintln!("Warning: Failed to create session file: {}", e);
        eprintln!("The bot will continue without session file caching.");
    }

    Arc::new(Env {
        discord_http: Arc::new(HttpBuilder::new(discord_token).build()),
        bot_singleton_handle: BotHandle::new(),
        llm: Arc::new((model, backend)),
        base_prompt: Arc::new(base_prompt),
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
