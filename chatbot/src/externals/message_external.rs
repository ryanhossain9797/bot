use std::sync::{Arc, LazyLock};

use crate::{
    externals::bash_container_external::{pull_file, PulledFile},
    services::telegram,
    types::conversation::{ConversationAction, Platform, ConversationId},
    Env,
};
use frankenstein::methods::{SendDocumentParams, SendMessageParams};
use frankenstein::AsyncTelegramApi;
use regex::Regex;
use serenity::all::{CreateAttachment, CreateMessage};

const DISCORD_MESSAGE_LIMIT: usize = 2000;
const TELEGRAM_MESSAGE_LIMIT: usize = 2048;

static ATTACH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\[attach_(?:file|image):\s*(.*?)\s*\]\]").expect("ATTACH_RE is a valid regex")
});

fn extract_attach_paths(text: &str) -> (String, Vec<String>) {
    let mut paths: Vec<String> = Vec::new();
    for cap in ATTACH_RE.captures_iter(text) {
        let path = cap[1].trim().to_string();
        if !path.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    let cleaned = ATTACH_RE.replace_all(text, "");
    let collapsed = Regex::new(r"[ \t]{2,}")
        .expect("static whitespace regex is valid")
        .replace_all(cleaned.trim(), " ")
        .to_string();
    (collapsed, paths)
}

fn split_message(content: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = content;

    while remaining.chars().count() > limit {
        let hard = remaining
            .char_indices()
            .nth(limit)
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
}

impl OutboundMessage {
    pub fn is_empty(&self) -> bool {
        self.message.is_none() && self.tool_names.is_empty()
    }
}

pub async fn send_message(
    env: Arc<Env>,
    conversation_id: ConversationId,
    outbound: OutboundMessage,
) -> ConversationAction {
    let container_key = conversation_id.to_string();
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

            let (mut text, attach_paths) = extract_attach_paths(&text);
            let mut files: Vec<CreateAttachment> = Vec::new();
            for path in &attach_paths {
                match pull_file(&container_key, path).await {
                    Ok(file) => files.push(CreateAttachment::bytes(file.bytes, file.filename)),
                    Err(err) => {
                        eprintln!("[send] attach '{path}' failed: {err}");
                        let note = Platform::Discord.subtext(&format!("couldn't attach {path}: {err}"));
                        text = if text.is_empty() { note } else { format!("{text}\n{note}") };
                    }
                }
            }

            let chunks = split_message(&text, DISCORD_MESSAGE_LIMIT);

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
        Platform::Telegram => {
            let result = send_telegram(&env, &container_key, &conversation_id.1, text).await;
            if let Err(err) = &result {
                eprintln!("[send] Telegram send failed: {err}");
            }
            ConversationAction::MessageSent(result)
        }
    }
}

async fn send_telegram(
    env: &Env,
    container_key: &str,
    key: &str,
    text: String,
) -> Result<(), String> {
    let bot = env
        .telegram
        .as_ref()
        .ok_or("Telegram is not configured")?;
    let (chat_id, thread_id) = telegram::parse_conversation_key(key)
        .ok_or_else(|| format!("invalid Telegram conversation key {key:?}"))?;

    let (mut text, attach_paths) = extract_attach_paths(&text);
    let mut files: Vec<PulledFile> = Vec::new();
    for path in &attach_paths {
        match pull_file(container_key, path).await {
            Ok(file) => files.push(file),
            Err(err) => {
                eprintln!("[send] attach '{path}' failed: {err}");
                let note = Platform::Telegram.subtext(&format!("couldn't attach {path}: {err}"));
                text = if text.is_empty() { note } else { format!("{text}\n{note}") };
            }
        }
    }

    for chunk in split_message(&text, TELEGRAM_MESSAGE_LIMIT) {
        let params = SendMessageParams::builder()
            .chat_id(chat_id)
            .maybe_message_thread_id(thread_id)
            .text(chunk)
            .build();
        bot.send_message(&params)
            .await
            .map_err(|err| telegram::describe(&err))?;
    }

    for file in files {
        let dir = std::env::temp_dir().join(format!("a2-upload-{}", uuid::Uuid::new_v4().simple()));
        let path = dir.join(&file.filename);
        let sent = async {
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|err| format!("staging {}: {err}", file.filename))?;
            tokio::fs::write(&path, &file.bytes)
                .await
                .map_err(|err| format!("staging {}: {err}", file.filename))?;
            let params = SendDocumentParams::builder()
                .chat_id(chat_id)
                .maybe_message_thread_id(thread_id)
                .document(path.clone())
                .build();
            bot.send_document(&params)
                .await
                .map(|_| ())
                .map_err(|err| telegram::describe(&err))
        }
        .await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        sent?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{extract_attach_paths, split_message};

    #[test]
    fn split_message_respects_the_limit_and_prefers_newlines() {
        let text = format!("{}\n{}", "a".repeat(6), "b".repeat(6));
        assert_eq!(split_message(&text, 10), vec!["aaaaaa\n", "bbbbbb"]);
        assert_eq!(split_message(&"é".repeat(5), 2), vec!["éé", "éé", "é"]);
        assert!(split_message("  \n ", 10).is_empty());
    }

    #[test]
    fn strips_marker_and_captures_path() {
        let (text, paths) =
            extract_attach_paths("Here's the report [[attach_file:/tmp/report.pdf]] enjoy");
        assert_eq!(text, "Here's the report enjoy");
        assert_eq!(paths, vec!["/tmp/report.pdf"]);
    }

    #[test]
    fn marker_only_message_becomes_empty_text() {
        let (text, paths) = extract_attach_paths("[[attach_image:chart.png]]");
        assert_eq!(text, "");
        assert_eq!(paths, vec!["chart.png"]);
    }

    #[test]
    fn file_and_image_markers_both_match_dedup_and_preserve_order() {
        let (text, paths) =
            extract_attach_paths("a [[attach_file:/x]] b [[attach_image:/y]] c [[attach_file:/x]]");
        assert_eq!(text, "a b c");
        assert_eq!(paths, vec!["/x", "/y"]);
    }

    #[test]
    fn tolerates_internal_whitespace_and_no_markers() {
        let (text, paths) = extract_attach_paths("[[attach_file:  /a b/c.txt  ]]");
        assert_eq!(paths, vec!["/a b/c.txt"]);
        assert!(text.is_empty());

        let (plain, none) = extract_attach_paths("just a normal message");
        assert_eq!(plain, "just a normal message");
        assert!(none.is_empty());
    }
}
