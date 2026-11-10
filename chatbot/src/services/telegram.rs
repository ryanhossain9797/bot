use std::sync::{Arc, LazyLock};
use std::time::Duration;

use frankenstein::client_reqwest::Bot;
use frankenstein::methods::{GetFileParams, GetUpdatesParams, SendDocumentParams, SendMessageParams};
use frankenstein::types::{AllowedUpdate, ChatType, Message, MessageEntity, MessageEntityType, User};
use frankenstein::updates::UpdateContent;
use frankenstein::{AsyncTelegramApi, Error};
use regex::Regex;

use crate::{
    externals::bash_container_external::PulledFile,
    state_machines::conversation_state_machine::ConversationMachine,
    types::conversation::{ConversationAction, ConversationConstructor, ConversationId, Platform},
    types::media::{Attachment, Image, MessageImage},
};

const POLL_TIMEOUT_SECS: u32 = 30;
const RETRY_DELAY: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_DOWNLOAD_BYTES: u64 = 20 * 1024 * 1024;
pub const NO_PUBLIC_URL: &str = "none (Telegram attachment)";

static WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("static whitespace regex is valid"));

#[derive(Clone)]
pub struct Telegram {
    bot: Bot,
    file_base: String,
}

impl Telegram {
    pub fn new(token: &str) -> Self {
        Self {
            bot: Bot::new(token),
            file_base: format!("https://api.telegram.org/file/bot{token}"),
        }
    }

    pub async fn send(
        &self,
        key: &str,
        chunks: Vec<String>,
        files: Vec<PulledFile>,
    ) -> Result<(), String> {
        let (chat_id, thread_id) = parse_conversation_key(key)
            .ok_or_else(|| format!("invalid Telegram conversation key {key:?}"))?;

        for chunk in chunks {
            let params = SendMessageParams::builder()
                .chat_id(chat_id)
                .maybe_message_thread_id(thread_id)
                .text(chunk)
                .build();
            self.bot
                .send_message(&params)
                .await
                .map_err(|err| describe(&err))?;
        }

        for file in files {
            let dir = std::env::temp_dir().join(format!("a2-upload-{}", uuid::Uuid::new_v4().simple()));
            let sent = self.send_document(chat_id, thread_id, &dir, &file).await;
            let _ = tokio::fs::remove_dir_all(&dir).await;
            sent?;
        }

        Ok(())
    }

    async fn send_document(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
        dir: &std::path::Path,
        file: &PulledFile,
    ) -> Result<(), String> {
        let staging = |err: std::io::Error| format!("staging {}: {err}", file.filename);
        let path = dir.join(&file.filename);
        tokio::fs::create_dir_all(dir).await.map_err(staging)?;
        tokio::fs::write(&path, &file.bytes).await.map_err(staging)?;
        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .maybe_message_thread_id(thread_id)
            .document(path)
            .build();
        self.bot
            .send_document(&params)
            .await
            .map(drop)
            .map_err(|err| describe(&err))
    }

    async fn download(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let file = self
            .bot
            .get_file(&GetFileParams::builder().file_id(file_id).build())
            .await
            .map_err(|err| describe(&err))?
            .result;
        let path = file.file_path.ok_or("Telegram returned no file path")?;
        let bytes = self
            .bot
            .client
            .get(format!("{}/{path}", self.file_base))
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|err| err.without_url().to_string())?
            .bytes()
            .await
            .map_err(|err| err.without_url().to_string())?;
        Ok(bytes.to_vec())
    }
}

pub async fn run_telegram(telegram: Telegram) -> anyhow::Result<()> {
    let bot_user = loop {
        match telegram.bot.get_me().await {
            Ok(response) => break response.result,
            Err(err) => {
                eprintln!("[telegram] getMe failed: {}; retrying", describe(&err));
                tokio::time::sleep(RETRY_DELAY).await;
            }
        }
    };
    println!(
        "[startup] telegram bot @{} ({})",
        bot_user.username.as_deref().unwrap_or("?"),
        bot_user.id
    );
    let poller = Poller { telegram, bot_user };

    let mut params = GetUpdatesParams::builder()
        .timeout(POLL_TIMEOUT_SECS)
        .allowed_updates(vec![AllowedUpdate::Message])
        .build();

    loop {
        match poller.telegram.bot.get_updates(&params).await {
            Ok(response) => {
                for update in response.result {
                    params.offset = Some(i64::from(update.update_id) + 1);
                    if let UpdateContent::Message(message) = update.content {
                        poller.handle(*message).await;
                    }
                }
            }
            Err(Error::JsonDecode { input, .. }) => {
                eprintln!("[telegram] could not decode a batch of updates; skipping it");
                match last_update_id(&input) {
                    Some(id) => params.offset = Some(id + 1),
                    None => tokio::time::sleep(RETRY_DELAY).await,
                }
            }
            Err(err) => {
                eprintln!("[telegram] getUpdates failed: {}", describe(&err));
                tokio::time::sleep(RETRY_DELAY).await;
            }
        }
    }
}

fn describe(err: &Error) -> String {
    match err {
        Error::Api(response) => format!("{} {}", response.error_code, response.description),
        Error::JsonDecode { source, .. } => format!("bad JSON from Telegram: {source}"),
        other => other.to_string(),
    }
}

fn last_update_id(input: &str) -> Option<i64> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    value
        .get("result")?
        .as_array()?
        .iter()
        .filter_map(|update| update.get("update_id")?.as_i64())
        .max()
}

fn conversation_key(chat_id: i64, thread_id: Option<i32>) -> String {
    match thread_id {
        Some(thread) => format!("{chat_id}/{thread}"),
        None => chat_id.to_string(),
    }
}

fn parse_conversation_key(key: &str) -> Option<(i64, Option<i32>)> {
    match key.split_once('/') {
        Some((chat, thread)) => Some((chat.parse().ok()?, Some(thread.parse().ok()?))),
        None => Some((key.parse().ok()?, None)),
    }
}

struct Poller {
    telegram: Telegram,
    bot_user: User,
}

impl Poller {
    async fn handle(&self, message: Message) {
        let Some(author) = message.from.as_deref() else {
            return;
        };
        if author.id == self.bot_user.id {
            return;
        }

        let attachments = self.collect_attachments(&message).await;
        let text = filter(&message, &self.bot_user);
        if text.is_none() && attachments.is_empty() {
            return;
        }

        let name = full_name(author);
        let msg = format!("{}: {}", identity(&name, author.id), text.unwrap_or_default());
        let thread_id = message
            .message_thread_id
            .filter(|_| message.is_topic_message == Some(true));

        let constructor = ConversationConstructor {
            id: ConversationId(
                Platform::Telegram,
                conversation_key(message.chat.id, thread_id),
            ),
            is_group: message.chat.type_field != ChatType::Private,
            bot_identity: identity(&self.bot_user.first_name, self.bot_user.id),
        };
        let action = ConversationAction::NewMessage {
            msg,
            user_id: author.id.to_string(),
            name,
            attachments,
        };

        re_framework::handle::<ConversationMachine>()
            .act_maybe_construct(constructor, action)
            .await;
    }

    async fn collect_attachments(&self, message: &Message) -> Vec<Attachment> {
        let mut attachments = Vec::new();

        if let Some(photo) = message
            .photo
            .as_deref()
            .and_then(|sizes| sizes.iter().max_by_key(|size| size.width * size.height))
        {
            let filename = format!("photo_{}.jpg", photo.file_unique_id);
            attachments.push(
                self.image_or_file(&photo.file_id, photo.file_size, filename, "image/jpeg".into())
                    .await,
            );
        }

        if let Some(document) = message.document.as_deref() {
            let filename = document
                .file_name
                .clone()
                .unwrap_or_else(|| format!("file_{}", document.file_unique_id));
            let image_mime = document
                .mime_type
                .clone()
                .filter(|mime| mime.starts_with("image/"));
            attachments.push(match image_mime {
                Some(mime) => {
                    self.image_or_file(&document.file_id, document.file_size, filename, mime)
                        .await
                }
                None => Attachment::File {
                    filename,
                    content_type: document.mime_type.clone(),
                    url: NO_PUBLIC_URL.to_string(),
                },
            });
        }

        attachments
    }

    async fn image_or_file(
        &self,
        file_id: &str,
        size: Option<u64>,
        filename: String,
        mime: String,
    ) -> Attachment {
        let downloaded = match size.is_some_and(|size| size > MAX_DOWNLOAD_BYTES) {
            true => Err("larger than the 20 MB Bot API download limit".to_string()),
            false => tokio::time::timeout(DOWNLOAD_TIMEOUT, self.telegram.download(file_id))
                .await
                .unwrap_or_else(|_| Err(format!("timed out after {DOWNLOAD_TIMEOUT:?}"))),
        };
        match downloaded {
            Ok(bytes) => Attachment::Image {
                image: MessageImage::Hydrated(Image {
                    bytes: Arc::new(bytes),
                    mime,
                }),
                filename,
                url: NO_PUBLIC_URL.to_string(),
            },
            Err(err) => {
                eprintln!("[telegram] failed to download image {filename}: {err}");
                Attachment::File {
                    filename,
                    content_type: Some(mime),
                    url: NO_PUBLIC_URL.to_string(),
                }
            }
        }
    }
}

fn full_name(user: &User) -> String {
    match &user.last_name {
        Some(last) => format!("{} {last}", user.first_name),
        None => user.first_name.clone(),
    }
}

fn identity(name: &str, id: u64) -> String {
    format!("({id}) {name}")
}

fn is_bot_username(handle: &str, bot_user: &User) -> bool {
    bot_user
        .username
        .as_deref()
        .is_some_and(|username| handle.eq_ignore_ascii_case(username))
}

fn rewrite(entity: &MessageEntity, span: &str, bot_user: &User) -> Option<String> {
    match entity.type_field {
        MessageEntityType::Mention => span
            .strip_prefix('@')
            .filter(|handle| is_bot_username(handle, bot_user))
            .map(|_| identity(&bot_user.first_name, bot_user.id)),
        MessageEntityType::TextMention => {
            let user = entity.user.as_ref()?;
            Some(identity(&full_name(user), user.id))
        }
        MessageEntityType::BotCommand => {
            let (command, handle) = span.split_once('@')?;
            is_bot_username(handle, bot_user).then(|| command.to_string())
        }
        _ => None,
    }
}

fn filter(message: &Message, bot_user: &User) -> Option<String> {
    let (raw, entities) = match &message.text {
        Some(text) => (text.as_str(), message.entities.as_deref()),
        None => (
            message.caption.as_deref().unwrap_or_default(),
            message.caption_entities.as_deref(),
        ),
    };

    let mut units: Vec<u16> = raw.encode_utf16().collect();
    let mut entities: Vec<&MessageEntity> = entities.unwrap_or_default().iter().collect();
    entities.sort_by_key(|entity| std::cmp::Reverse(entity.offset));
    for entity in entities {
        let start = usize::from(entity.offset);
        let end = start + usize::from(entity.length);
        let Some(span) = units.get(start..end).map(String::from_utf16_lossy) else {
            continue;
        };
        if let Some(replacement) = rewrite(entity, &span, bot_user) {
            units.splice(start..end, replacement.encode_utf16());
        }
    }
    let text = String::from_utf16_lossy(&units);

    let text = text.trim().trim_start_matches('/').trim();
    let text = WHITESPACE.replace_all(text, " ").trim().to_string();

    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bot() -> User {
        serde_json::from_value(serde_json::json!({
            "id": 9, "is_bot": true, "first_name": "A2", "username": "a2bot",
        }))
        .unwrap()
    }

    fn message(body: serde_json::Value) -> Message {
        let mut base = serde_json::json!({
            "message_id": 1, "date": 0, "chat": {"id": -100, "type": "supergroup"},
        });
        base.as_object_mut()
            .unwrap()
            .extend(body.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn conversation_keys_round_trip_with_and_without_a_topic() {
        assert_eq!(conversation_key(-1001234, None), "-1001234");
        assert_eq!(conversation_key(-1001234, Some(7)), "-1001234/7");
        assert_eq!(parse_conversation_key("-1001234"), Some((-1001234, None)));
        assert_eq!(parse_conversation_key("-1001234/7"), Some((-1001234, Some(7))));
        assert_eq!(parse_conversation_key("abc"), None);
        assert_eq!(parse_conversation_key("1/x"), None);
    }

    #[test]
    fn bot_mentions_and_commands_use_entities() {
        let m = message(serde_json::json!({
            "text": "/ask@A2Bot   hey @a2bot, ask @someone",
            "entities": [
                {"type": "bot_command", "offset": 0, "length": 10},
                {"type": "mention", "offset": 17, "length": 6},
                {"type": "mention", "offset": 29, "length": 8},
            ],
        }));
        assert_eq!(
            filter(&m, &bot()).as_deref(),
            Some("ask hey (9) A2, ask @someone")
        );
    }

    #[test]
    fn text_mentions_rewrite_the_span_they_point_at() {
        let m = message(serde_json::json!({
            "text": "Al, tell Zoë 🍷 and Al hi",
            "entities": [
                {"type": "text_mention", "offset": 9, "length": 6,
                 "user": {"id": 5, "is_bot": false, "first_name": "Zoë", "last_name": "🍷"}},
                {"type": "text_mention", "offset": 20, "length": 2,
                 "user": {"id": 6, "is_bot": false, "first_name": "Al"}},
            ],
        }));
        assert_eq!(
            filter(&m, &bot()).as_deref(),
            Some("Al, tell (5) Zoë 🍷 and (6) Al hi")
        );
    }

    #[test]
    fn caption_is_used_when_there_is_no_text_and_blank_is_none() {
        let captioned = message(serde_json::json!({"caption": " look  at this "}));
        assert_eq!(filter(&captioned, &bot()).as_deref(), Some("look at this"));
        let blank = message(serde_json::json!({"photo": []}));
        assert_eq!(filter(&blank, &bot()), None);
    }

    #[test]
    fn undecodable_batches_are_skipped_past_their_last_update() {
        let input = r#"{"ok":true,"result":[{"update_id":41,"x":{}},{"update_id":43,"y":1}]}"#;
        assert_eq!(last_update_id(input), Some(43));
        assert_eq!(last_update_id("not json"), None);
    }
}
