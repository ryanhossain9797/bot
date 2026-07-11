use std::sync::Arc;

use crate::{
    types::conversation::{ConversationAction, Platform, ConversationId},
    types::media::MessageImage,
    Env,
};
use serenity::all::{CreateAttachment, CreateMessage};

const DISCORD_MESSAGE_LIMIT: usize = 2000;

fn split_for_discord(content: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = content;

    while remaining.chars().count() > DISCORD_MESSAGE_LIMIT {
        let hard = remaining
            .char_indices()
            .nth(DISCORD_MESSAGE_LIMIT)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len());
        let split_at = remaining[..hard].rfind('\n').map(|i| i + 1).unwrap_or(hard);

        let (chunk, rest) = remaining.split_at(split_at);
        if !chunk.trim().is_empty() {
            chunks.push(chunk.to_string());
        }
        remaining = rest;
    }

    if !remaining.trim().is_empty() {
        chunks.push(remaining.to_string());
    }
    chunks
}

fn compose(platform: &Platform, message: Option<String>, tool_names: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(m) = message {
        parts.push(m);
    }
    if !tool_names.is_empty() {
        let body = match tool_names {
            [one] => format!("using tool: {one}"),
            many => format!("using multiple tools: {}", many.join(", ")),
        };
        parts.push(platform.subtext(&body));
    }
    parts.join("\n")
}

pub struct OutboundMessage {
    pub message: Option<String>,
    pub tool_names: Vec<String>,
    pub attachments: Vec<Attachment>,
}

pub enum Attachment {
    Image(MessageImage),
}

impl OutboundMessage {
    pub fn is_empty(&self) -> bool {
        self.message.is_none() && self.tool_names.is_empty() && self.attachments.is_empty()
    }
}

fn discord_attachment(attachment: &Attachment, index: usize) -> Option<CreateAttachment> {
    let image = match attachment {
        Attachment::Image(image) => image,
    };
    let MessageImage::Hydrated(image) = image else {
        return None;
    };
    let ext = match image.mime.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    };
    Some(CreateAttachment::bytes((*image.bytes).clone(), format!("attachment_{index}.{ext}")))
}

pub async fn send_message(
    env: Arc<Env>,
    conversation_id: ConversationId,
    outbound: OutboundMessage,
) -> ConversationAction {
    let text = compose(&conversation_id.0, outbound.message, &outbound.tool_names);
    match conversation_id.0 {
        Platform::Discord => {
            let channel = match conversation_id.1.parse::<u64>() {
                Ok(id) => serenity::all::ChannelId::new(id),
                Err(err) => {
                    eprintln!("[send] invalid channel id {:?}: {err}", conversation_id.1);
                    return ConversationAction::MessageSent(Err(err.to_string()));
                }
            };

            let files: Vec<CreateAttachment> = outbound
                .attachments
                .iter()
                .enumerate()
                .filter_map(|(i, a)| discord_attachment(a, i))
                .collect();

            let chunks = split_for_discord(&text);

            if chunks.is_empty() {
                if !files.is_empty() {
                    if let Err(err) = channel
                        .send_message(&env.discord_http, CreateMessage::new().files(files))
                        .await
                    {
                        eprintln!("[send] Discord send failed: {err}");
                        return ConversationAction::MessageSent(Err(err.to_string()));
                    }
                }
                return ConversationAction::MessageSent(Ok(()));
            }

            let last = chunks.len() - 1;
            let mut files = Some(files);
            for (i, chunk) in chunks.into_iter().enumerate() {
                let mut builder = CreateMessage::new().content(&chunk);
                if i == last {
                    if let Some(files) = files.take().filter(|f| !f.is_empty()) {
                        builder = builder.files(files);
                    }
                }
                if let Err(err) = channel.send_message(&env.discord_http, builder).await {
                    eprintln!("[send] Discord send failed: {err}");
                    return ConversationAction::MessageSent(Err(err.to_string()));
                }
            }
            ConversationAction::MessageSent(Ok(()))
        }
        _ => panic!("Telegram not yet implemented"),
    }
}
