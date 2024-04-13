use common::get_client_token;

use regex::Regex;
use serenity::{async_trait, model::channel::Message as DMessage, prelude::*};

use crate::models::{
    bot::{BotAction, BotHandle},
    user::{UserChannel, UserId},
};

use super::common;

///Main Starting point for the Discord api.
pub async fn run_discord(bot_handle: BotHandle) -> anyhow::Result<()> {
    // Configure the client with your Discord bot token in the environment.
    let token = get_client_token("discord_token")
        .ok_or_else(|| anyhow::anyhow!("Failed to load Discord Token"))?;

    let intents = GatewayIntents::DIRECT_MESSAGES;

    // Create a new instance of the Client, logging in as a bot. This will
    let mut client = Client::builder(token, intents)
        .event_handler(Handler { bot_handle })
        .await?;

    // Finally, start a single shard, and start listening to events
    // Shards will automatically attempt to reconnect, and will perform
    // exponential backoff until it reconnects.
    client.start().await?;
    Err(anyhow::anyhow!("Discord failed"))
}

struct Handler {
    bot_handle: BotHandle,
}

#[async_trait]
impl EventHandler for Handler {
    // Set a handler for the `message` event - so that whenever a new message
    // is received - the closure (or function) passed will be called.
    async fn message(&self, ctx: Context, message: DMessage) {
        if !message.author.bot {
            if let Some((msg, start_conversation)) = filter(&message, &ctx).await {
                let action = BotAction::HandleMessage {
                    user_id: UserId(UserChannel::Discord, message.author.id.get().to_string()),
                    start_conversation,
                    msg,
                };
                self.bot_handle.act(action).await;
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
    let source = "DISCORD";

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
        .to_lowercase();

    let space_trimmer = Regex::new(r"\s+").unwrap();

    let msg: String = space_trimmer.replace_all(&msg, " ").into();
    //-----------------------check if message is from a group chat.......
    Some((
        msg,
        message.is_private() || message.content.contains(handle.as_str()),
    ))
}
