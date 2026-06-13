use std::sync::Arc;

use crate::{
    types::conversation::{ConversationAction, Platform, ConversationId},
    Env,
};
use serenity::all::CreateMessage;

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

pub async fn send_message(env: Arc<Env>, conversation_id: ConversationId, message: String) -> ConversationAction {
    match conversation_id.0 {
        Platform::Discord => {
            let channel = match conversation_id.1.parse::<u64>() {
                Ok(id) => serenity::all::ChannelId::new(id),
                Err(err) => {
                    eprintln!("[send] invalid channel id {:?}: {err}", conversation_id.1);
                    return ConversationAction::MessageSent(Err(err.to_string()));
                }
            };

            for chunk in split_for_discord(&message) {
                if let Err(err) = channel
                    .send_message(&env.discord_http, CreateMessage::new().content(&chunk))
                    .await
                {
                    eprintln!("[send] Discord send failed: {err}");
                    return ConversationAction::MessageSent(Err(err.to_string()));
                }
            }
            ConversationAction::MessageSent(Ok(()))
        }
        _ => panic!("Telegram not yet implemented"),
    }
}
