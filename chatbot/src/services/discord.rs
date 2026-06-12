use re_framework::StateMachine;
use regex::Regex;
use serenity::{async_trait, model::channel::Message as DMessage, prelude::*};

use crate::{
    state_machines::conversation_state_machine::ConversationMachine,
    types::conversation::{ConversationAction, ConversationConstructor, ConversationId, Platform},
};

pub async fn prepare_discord_client(discord_token: &str) -> anyhow::Result<Client> {
    let intents = GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let http = serenity::all::HttpBuilder::new(discord_token).build();
    let bot_user = http.get_current_user().await?;
    let bot_user_id = bot_user.id.get();
    let bot_name = bot_user
        .global_name
        .clone()
        .unwrap_or_else(|| bot_user.name.clone());

    let client = Client::builder(discord_token, intents)
        .event_handler(Handler {
            bot_user_id,
            bot_name,
        })
        .await?;

    Ok(client)
}

pub async fn run_discord(mut client: Client) -> anyhow::Result<()> {
    client.start().await?;
    Err(anyhow::anyhow!("Discord failed"))
}

struct Handler {
    bot_user_id: u64,
    bot_name: String,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, _ctx: Context, message: DMessage) {
        if message.author.id.get() == self.bot_user_id {
            return;
        }
        let Some(text) = filter(&message, self.bot_user_id, &self.bot_name) else {
            return;
        };

        let is_group = message.guild_id.is_some();
        let author_id = message.author.id.get();
        let user_id = author_id.to_string();
        let name = message
            .author
            .global_name
            .clone()
            .unwrap_or_else(|| message.author.name.clone());

        let msg = format!("{}: {text}", identity(&name, author_id));

        let bot_identity = identity(&self.bot_name, self.bot_user_id);

        let conversation_id =
            ConversationId(Platform::Discord, message.channel_id.get().to_string());

        let handle = ConversationMachine::handle();

        handle
            .maybe_construct(ConversationConstructor {
                id: conversation_id.clone(),
                is_group,
                bot_identity,
            })
            .await;

        let action = ConversationAction::NewMessage { msg, user_id, name };

        handle.act(conversation_id, action).await;
    }
}

fn identity(name: &str, id: u64) -> String {
    format!("{name} (id:{id})")
}

fn filter(message: &DMessage, bot_user_id: u64, bot_name: &str) -> Option<String> {
    let mut text = message.content.clone();
    for user in &message.mentions {
        let id = user.id.get();
        let name = if id == bot_user_id {
            bot_name.to_string()
        } else {
            user.global_name
                .clone()
                .unwrap_or_else(|| user.name.clone())
        };
        let label = identity(&name, id);
        text = text
            .replace(&format!("<@{id}>"), &label)
            .replace(&format!("<@!{id}>"), &label);
    }

    let text = text.trim().trim_start_matches('/').trim().to_string();
    let space_trimmer = Regex::new(r"\s+").expect("static whitespace regex is valid");
    let text: String = space_trimmer.replace_all(&text, " ").trim().to_string();

    (!text.is_empty()).then_some(text)
}
