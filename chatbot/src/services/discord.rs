use framework::StateMachineHandle;
use regex::Regex;
use serenity::{async_trait, model::channel::Message as DMessage, prelude::*};

use crate::{
    types::conversation::{ConversationAction, Platform, ConversationId},
    state_machines::conversation_state_machine::CONVERSATION_STATE_MACHINE,
};

pub async fn prepare_discord_client(discord_token: &str) -> anyhow::Result<Client> {
    // Configure the client with your Discord bot token in the environment.

    // DMs + group/server channels. MESSAGE_CONTENT is a privileged intent (must also be enabled in
    // the Discord Developer Portal) — required to read the text of messages that don't mention us.
    let intents = GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let conversation_state_machine = CONVERSATION_STATE_MACHINE.clone();

    // Create a new instance of the Client, logging in as a bot. This will
    let client = Client::builder(discord_token, intents)
        .event_handler(Handler { conversation_state_machine })
        .await?;

    Ok(client)
}

///Main Starting point for the Discord api.
pub async fn run_discord(mut client: Client) -> anyhow::Result<()> {
    client.start().await?;
    Err(anyhow::anyhow!("Discord failed"))
}

struct Handler {
    conversation_state_machine: StateMachineHandle<ConversationId, ConversationAction>,
}

#[async_trait]
impl EventHandler for Handler {
    // Set a handler for the `message` event - so that whenever a new message
    // is received - the closure (or function) passed will be called.
    async fn message(&self, ctx: Context, message: DMessage) {
        let env = crate::ENV.get().expect("ENV initialized before clients start");

        // Ignore only our OWN messages (matched by id), not all bots — so the bot still sees and can
        // react to other bots in the channel. (Our own replies are already in history as assistant
        // turns; re-ingesting them as user input would double them and risk a self-loop.)
        if message.author.id.get() == env.bot_user_id {
            return;
        }
        let Some(text) = filter(&message, env.bot_user_id, &env.bot_name) else {
            return;
        };

        let is_group = message.guild_id.is_some();
        let user_id = message.author.id.get().to_string();
        // Display name: a guild nick if set, else the global/display name, else the username (no nick
        // in a DM). Name resolution lives only in this adapter — the domain layer never sees it.
        let name = match message.guild_id {
            Some(_) => message
                .author_nick(&ctx)
                .await
                .or_else(|| message.author.global_name.clone())
                .unwrap_or_else(|| message.author.name.clone()),
            None => message
                .author
                .global_name
                .clone()
                .unwrap_or_else(|| message.author.name.clone()),
        };

        // Prefix the sender's name onto the text — for every message, DM or group — so the model
        // always knows who is speaking (every human maps to OpenAI role "user", so the name in the
        // content is the only speaker signal).
        let msg = format!("{name}: {text}");

        // Key the conversation by the channel the message arrived on (a DM channel is 1:1, a server
        // channel is shared). The channel id is stored as the opaque conversation id string; only
        // this Discord adapter knows it's a channel id.
        let conversation_id =
            ConversationId(Platform::Discord, message.channel_id.get().to_string());
        let action = ConversationAction::NewMessage {
            msg,
            user_id,
            name,
            is_group,
        };
        self.conversation_state_machine
            .act(conversation_id, action)
            .await;
    }
}

/// Clean a raw message into the text we feed the model, or `None` to ignore it:
/// - replaces the bot's own `@mention` (both `<@id>` and `<@!id>` forms) with its **name**, so the
///   model can see when it's being addressed instead of the mention being silently stripped,
/// - strips a leading `/`, collapses runs of whitespace,
/// - returns `None` if nothing textual remains (e.g. an attachment-only message), so we don't run
///   the model on empty content.
fn filter(message: &DMessage, bot_user_id: u64, bot_name: &str) -> Option<String> {
    let mut text = message.content.clone();
    for handle in [format!("<@{bot_user_id}>"), format!("<@!{bot_user_id}>")] {
        text = text.replace(&handle, bot_name);
    }

    let text = text.trim().trim_start_matches('/').trim().to_string();
    let space_trimmer = Regex::new(r"\s+").unwrap();
    let text: String = space_trimmer.replace_all(&text, " ").trim().to_string();

    (!text.is_empty()).then_some(text)
}
