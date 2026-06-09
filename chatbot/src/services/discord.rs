use framework::StateMachineHandle;
use regex::Regex;
use serenity::{async_trait, model::channel::Message as DMessage, prelude::*};

use crate::{
    types::conversation::{ConversationAction, ConversationConstructor, Platform, ConversationId},
    state_machines::conversation_state_machine::CONVERSATION_STATE_MACHINE,
};

pub async fn prepare_discord_client(discord_token: &str) -> anyhow::Result<Client> {
    // Configure the client with your Discord bot token in the environment.

    // DMs + group/server channels. MESSAGE_CONTENT is a privileged intent (must also be enabled in
    // the Discord Developer Portal) — required to read the text of messages that don't mention us.
    let intents = GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    // Fetch our own Discord identity up front (id + display name). This is adapter-local — bot
    // identity is per-platform, not a global Env concept, so each platform's adapter holds its own.
    let http = serenity::all::HttpBuilder::new(discord_token).build();
    let bot_user = http.get_current_user().await?;
    let bot_user_id = bot_user.id.get();
    let bot_name = bot_user
        .global_name
        .clone()
        .unwrap_or_else(|| bot_user.name.clone());

    let conversation_state_machine = CONVERSATION_STATE_MACHINE.clone();

    // Create a new instance of the Client, logging in as a bot. This will
    let client = Client::builder(discord_token, intents)
        .event_handler(Handler {
            conversation_state_machine,
            bot_user_id,
            bot_name,
        })
        .await?;

    Ok(client)
}

///Main Starting point for the Discord api.
pub async fn run_discord(mut client: Client) -> anyhow::Result<()> {
    client.start().await?;
    Err(anyhow::anyhow!("Discord failed"))
}

struct Handler {
    conversation_state_machine:
        StateMachineHandle<ConversationId, ConversationConstructor, ConversationAction>,
    /// This bot's own Discord identity (adapter-local — bot identity is per-platform, never a global
    /// Env value). Used to ignore our own messages, humanize our own @mentions, and stamp the
    /// `bot_identity` carried on each message into the domain.
    bot_user_id: u64,
    bot_name: String,
}

#[async_trait]
impl EventHandler for Handler {
    // Set a handler for the `message` event - so that whenever a new message
    // is received - the closure (or function) passed will be called.
    async fn message(&self, _ctx: Context, message: DMessage) {
        // Ignore only our OWN messages (matched by id), not all bots — so the bot still sees and can
        // react to other bots in the channel. (Our own replies are already in history as assistant
        // turns; re-ingesting them as user input would double them and risk a self-loop.)
        if message.author.id.get() == self.bot_user_id {
            return;
        }
        let Some(text) = filter(&message, self.bot_user_id, &self.bot_name) else {
            return;
        };

        let is_group = message.guild_id.is_some();
        let author_id = message.author.id.get();
        let user_id = author_id.to_string();
        // Display name straight from the message payload (global/display name, else username). No
        // guild-nick lookup: that would be a blocking HTTP member fetch on every group message (we
        // don't request GUILD_MEMBERS, so it's never cached), which can stall the whole inbound path
        // under rate limiting. Nicks can be re-added later via the cache without a blocking call.
        let name = message
            .author
            .global_name
            .clone()
            .unwrap_or_else(|| message.author.name.clone());

        // Prefix the sender's identity — name AND Discord id — onto the text, for every message, DM
        // or group. The id makes the speaker unambiguous: names can collide or change, and the model
        // needs a stable handle to tell who's who and whether a mention refers to itself (see
        // `identity` / mention humanization in `filter`).
        let msg = format!("{}: {text}", identity(&name, author_id));

        // The bot's own identity on this platform, stamped onto the message so the domain/LLM path
        // knows it per-conversation without a global Env value (identity is per-platform).
        let bot_identity = identity(&self.bot_name, self.bot_user_id);

        // Key the conversation by the channel the message arrived on (a DM channel is 1:1, a server
        // channel is shared). The channel id is stored as the opaque conversation id string; only
        // this Discord adapter knows it's a channel id.
        let conversation_id =
            ConversationId(Platform::Discord, message.channel_id.get().to_string());

        // Construct-then-act: ensure the conversation exists (idempotent — only the first message on
        // this channel actually creates it, baking in the group-vs-DM context and our identity),
        // then deliver the message. `is_group`/`bot_identity` are conversation facts, set once here.
        self.conversation_state_machine
            .construct(
                conversation_id.clone(),
                ConversationConstructor {
                    is_group,
                    bot_identity,
                },
            )
            .await;
        let action = ConversationAction::NewMessage {
            msg,
            user_id,
            name,
        };
        self.conversation_state_machine
            .act(conversation_id, action)
            .await;
    }
}

/// A person's stable identifier as the model sees it: name plus Discord id, e.g.
/// `Zireael9797 (id:12345)`. Used for both message prefixes and humanized @mentions so the model
/// can always correlate who's who — and tell when an id matches its own. Deliberately *not* the
/// `<@id>` ping form, so the bot echoing an identity can't accidentally ping anyone.
fn identity(name: &str, id: u64) -> String {
    format!("{name} (id:{id})")
}

/// Clean a raw message into the text we feed the model, or `None` to ignore it:
/// - rewrites every `@mention` (`<@id>` / `<@!id>`) into the mentioned user's `identity` (name+id),
///   so no raw numeric id ever reaches the model with no name attached,
/// - strips a leading `/`, collapses runs of whitespace,
/// - returns `None` if nothing textual remains (e.g. an attachment-only message), so we don't run
///   the model on empty content.
fn filter(message: &DMessage, bot_user_id: u64, bot_name: &str) -> Option<String> {
    let mut text = message.content.clone();
    for user in &message.mentions {
        let id = user.id.get();
        // Use our configured name for ourselves; otherwise the mentioned user's display name.
        let name = if id == bot_user_id {
            bot_name.to_string()
        } else {
            user.global_name.clone().unwrap_or_else(|| user.name.clone())
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
