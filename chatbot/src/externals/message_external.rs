use std::sync::Arc;

use crate::types::user::UserAction;
use crate::types::user::UserChannel;
use crate::{types::user::UserId, Env};
use serenity::all::CreateMessage;

/// Discord rejects message content longer than 2000 characters.
const DISCORD_MESSAGE_LIMIT: usize = 2000;

/// Split `content` into chunks of at most `DISCORD_MESSAGE_LIMIT` characters, preferring to break
/// at a newline so we don't cut mid-line. Empty/whitespace-only chunks are dropped (Discord also
/// rejects empty content).
fn split_for_discord(content: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = content;

    while remaining.chars().count() > DISCORD_MESSAGE_LIMIT {
        // Byte index of the character at the limit (a valid char boundary).
        let hard = remaining
            .char_indices()
            .nth(DISCORD_MESSAGE_LIMIT)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len());
        // Break at the last newline within the window, else hard-cut at the limit.
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

pub async fn send_message(env: Arc<Env>, user_id: UserId, message: String) -> UserAction {
    let user_id_result = match user_id.0 {
        UserChannel::Discord => {
            let user_id_result = user_id.1.parse::<u64>();
            match user_id_result {
                Ok(user_id) => Ok(serenity::all::UserId::new(user_id)),
                Err(err) => Err(err.to_string()),
            }
        }
        _ => panic!("Telegram not yet implemented"),
    };

    match user_id_result {
        Err(err) => UserAction::MessageSent(Err(err)),
        Ok(user_id) => {
            let dm_channel_result = match user_id.to_user(&env.discord_http).await {
                Ok(user) => user.create_dm_channel(&env.discord_http).await,
                Err(e) => Err(e),
            };

            match dm_channel_result {
                Ok(channel) => {
                    // Chunk to Discord's 2000-char limit; send sequentially, bail on first error.
                    for chunk in split_for_discord(&message) {
                        if let Err(err) = channel
                            .send_message(&env.discord_http, CreateMessage::new().content(&chunk))
                            .await
                        {
                            eprintln!("Failed to send Discord message: {err}");
                            return UserAction::MessageSent(Err(err.to_string()));
                        }
                    }
                    UserAction::MessageSent(Ok(()))
                }
                Err(err) => UserAction::MessageSent(Err(err.to_string())),
            }
        }
    }
}
