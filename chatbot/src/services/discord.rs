use framework::StateMachineHandle;
use regex::Regex;
use serenity::{async_trait, model::channel::Message as DMessage, prelude::*};

use crate::{
    types::conversation::{ConversationAction, Platform, ConversationId},
    state_machines::conversation_state_machine::CONVERSATION_STATE_MACHINE,
};

pub async fn prepare_discord_client(discord_token: &str) -> anyhow::Result<Client> {
    // Configure the client with your Discord bot token in the environment.

    let intents = GatewayIntents::DIRECT_MESSAGES;

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
        if !message.author.bot {
            if let Some((msg, start_conversation)) = filter(&message, &ctx).await {
                // Key the conversation by the channel the message arrived on (a DM channel is 1:1,
                // a server channel is shared). The channel id is stored as the opaque conversation
                // id string; only this Discord adapter knows it's a channel id.
                let conversation_id = ConversationId(Platform::Discord, message.channel_id.get().to_string());
                let action = ConversationAction::NewMessage {
                    start_conversation,
                    msg,
                };
                self.conversation_state_machine.act(conversation_id, action).await;
            }
        }
    }
}

///Filter basically does some spring cleaning.
/// - checks whether the update is actually a message or some other type.
/// - trims leading and trailing spaces ("   /hellow    @machinelifeformbot   world  " becomes "/hellow    @machinelifeformbot   world").
/// - removes / from start if it's there ("/hellow    @machinelifeformbot   world" becomes "hellow    @machinelifeformbot   world").
/// - removes mentions of the bot from the message ("hellow    @machinelifeformbot   world" becomes "hellow      world").
/// - replaces redundant spaces with single spaces using regex ("hellow      world" becomes "hellow world").
async fn filter(message: &DMessage, ctx: &Context) -> Option<(String, bool)> {
    let Ok(info) = ctx.http.get_current_application_info().await else {
        return None;
    };

    let id: i64 = info.id.into();
    //-----------------------remove self mention from message
    let handle = format!("<@{}>", &id);

    let msg = message
        .content
        .replace(handle.as_str(), "")
        .trim()
        .trim_start_matches('/')
        .trim()
        .to_string();

    let space_trimmer = Regex::new(r"\s+").unwrap();

    let msg: String = space_trimmer.replace_all(&msg, " ").into();
    //-----------------------check if message is from a group chat.......
    Some((
        msg,
        message.guild_id.is_none() || message.content.contains(handle.as_str()),
    ))
}
