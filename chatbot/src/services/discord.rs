use std::sync::Arc;

use serenity::all::{
    ChannelId, CreateAllowedMentions, CreateAttachment, CreateMessage, Http, HttpBuilder,
    MessageType, User,
};
use serenity::{async_trait, model::channel::Message as DMessage, prelude::*};

use crate::{
    services::{Delivery, deliver, normalize_text},
    types::conversation::{ConversationId, Identity, IncomingMessage, Platform},
    types::media::{Attachment, OutgoingFile},
    types::mention::{Segment, mention_segments},
};

#[derive(Clone)]
pub struct Discord {
    http: Arc<Http>,
    token: String,
}

impl Discord {
    pub fn new(token: &str) -> Self {
        Self {
            http: Arc::new(HttpBuilder::new(token).build()),
            token: token.to_string(),
        }
    }

    pub async fn client(&self) -> anyhow::Result<Client> {
        let intents = GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let bot_user = self.http.get_current_user().await?;
        let bot = Identity::new(bot_user.id.get(), display_name(&bot_user));

        Ok(Client::builder(&self.token, intents)
            .event_handler(Handler { bot })
            .await?)
    }

    pub async fn send(
        &self,
        channel_key: &str,
        chunks: Vec<String>,
        files: Vec<OutgoingFile>,
    ) -> Result<(), String> {
        let channel = channel_key
            .parse::<u64>()
            .map(ChannelId::new)
            .map_err(|err| format!("invalid channel id {channel_key:?}: {err}"))?;
        let files: Vec<CreateAttachment> = files
            .into_iter()
            .map(|file| CreateAttachment::bytes(file.bytes, file.filename))
            .collect();

        let mut messages: Vec<CreateMessage> = chunks
            .iter()
            .map(|chunk| new_message().content(render_mentions(chunk)))
            .collect();
        if !files.is_empty() {
            let last = messages.pop().unwrap_or_else(new_message);
            messages.push(last.files(files));
        }

        for message in messages {
            channel
                .send_message(&self.http, message)
                .await
                .map_err(|err| err.to_string())?;
        }
        Ok(())
    }
}

pub async fn run_discord(mut client: Client) -> anyhow::Result<()> {
    client.start().await?;
    Err(anyhow::anyhow!("Discord failed"))
}

fn new_message() -> CreateMessage {
    CreateMessage::new().allowed_mentions(CreateAllowedMentions::new().all_users(true))
}

fn render_mentions(text: &str) -> String {
    mention_segments(text)
        .into_iter()
        .map(|segment| match segment {
            Segment::Text(text) => text.to_string(),
            Segment::Id(id) => format!("<@{id}>"),
            Segment::Handle(handle) => format!("@{handle}"),
        })
        .collect()
}

fn display_name(user: &User) -> String {
    user.global_name.clone().unwrap_or_else(|| user.name.clone())
}

struct Handler {
    bot: Identity,
}

impl Handler {
    fn identity(&self, user: &User) -> Identity {
        match user.id.get() == self.bot.id {
            true => self.bot.clone(),
            false => Identity::new(user.id.get(), display_name(user)),
        }
    }

    fn replied_to(&self, message: &DMessage) -> Option<Identity> {
        if message.kind != MessageType::InlineReply {
            return None;
        }
        Some(self.identity(&message.referenced_message.as_deref()?.author))
    }

    fn filter(&self, message: &DMessage) -> Option<String> {
        let mut text = message.content.clone();
        for user in &message.mentions {
            let id = user.id.get();
            let label = self.identity(user).to_string();
            text = text
                .replace(&format!("<@{id}>"), &label)
                .replace(&format!("<@!{id}>"), &label);
        }
        normalize_text(&text)
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, _ctx: Context, message: DMessage) {
        if message.author.id.get() == self.bot.id {
            return;
        }

        let incoming = IncomingMessage {
            author: self.identity(&message.author),
            replied_to: self.replied_to(&message),
            text: self.filter(&message),
            attachments: collect_attachments(&message).await,
        };
        deliver(Delivery {
            conversation: ConversationId(Platform::Discord, message.channel_id.get().to_string()),
            is_group: message.guild_id.is_some(),
            bot: self.bot.clone(),
            message: incoming,
        })
        .await;
    }
}

async fn collect_attachments(message: &DMessage) -> Vec<Attachment> {
    let mut attachments = Vec::new();
    for attachment in &message.attachments {
        let filename = attachment.filename.clone();
        let url = Some(attachment.url.clone());
        let image_mime = attachment
            .content_type
            .clone()
            .filter(|mime| mime.starts_with("image/"));
        attachments.push(match image_mime {
            Some(mime) => {
                let download = attachment.download().await.map_err(|err| err.to_string());
                Attachment::image_or_file(download, filename, mime, url)
            }
            None => Attachment::File {
                filename,
                content_type: attachment.content_type.clone(),
                url,
            },
        });
    }
    attachments
}

#[cfg(test)]
mod tests {
    use super::render_mentions;

    #[test]
    fn ids_stay_native_and_handles_become_plain_text() {
        assert_eq!(
            render_mentions("<@!123> meet <@bob_k>, not <@everyone> <#5>"),
            "<@123> meet @bob_k, not @everyone <#5>"
        );
    }
}
