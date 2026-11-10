use std::sync::Arc;
use std::time::Duration;

use frankenstein::client_reqwest::Bot;
use frankenstein::methods::{GetFileParams, GetUpdatesParams};
use frankenstein::types::{AllowedUpdate, ChatType, Message, MessageEntityType, User};
use frankenstein::updates::UpdateContent;
use frankenstein::{AsyncTelegramApi, Error};
use regex::Regex;

use crate::{
    state_machines::conversation_state_machine::ConversationMachine,
    types::conversation::{ConversationAction, ConversationConstructor, ConversationId, Platform},
    types::media::{Attachment, Image, MessageImage},
};

const POLL_TIMEOUT_SECS: u32 = 30;
const RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_DOWNLOAD_BYTES: u64 = 20 * 1024 * 1024;
pub const NO_PUBLIC_URL: &str = "none (Telegram attachment)";

pub struct TelegramClient {
    bot: Bot,
    token: String,
    bot_user: User,
}

pub fn bot(token: &str) -> Bot {
    Bot::new(token)
}

pub async fn prepare_telegram_client(token: &str) -> anyhow::Result<TelegramClient> {
    let bot = bot(token);
    let bot_user = bot
        .get_me()
        .await
        .map_err(|err| anyhow::anyhow!("Telegram getMe failed: {}", describe(&err)))?
        .result;
    println!(
        "[startup] telegram bot @{} ({})",
        bot_user.username.as_deref().unwrap_or("?"),
        bot_user.id
    );
    Ok(TelegramClient {
        bot,
        token: token.to_string(),
        bot_user,
    })
}

pub async fn run_telegram(client: TelegramClient) -> anyhow::Result<()> {
    let mut params = GetUpdatesParams::builder()
        .timeout(POLL_TIMEOUT_SECS)
        .allowed_updates(vec![AllowedUpdate::Message])
        .build();

    loop {
        match client.bot.get_updates(&params).await {
            Ok(response) => {
                for update in response.result {
                    params.offset = Some(i64::from(update.update_id) + 1);
                    if let UpdateContent::Message(message) = update.content {
                        client.handle(*message).await;
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

pub fn describe(err: &Error) -> String {
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

pub fn conversation_key(chat_id: i64, thread_id: Option<i32>) -> String {
    match thread_id {
        Some(thread) => format!("{chat_id}/{thread}"),
        None => chat_id.to_string(),
    }
}

pub fn parse_conversation_key(key: &str) -> Option<(i64, Option<i32>)> {
    match key.split_once('/') {
        Some((chat, thread)) => Some((chat.parse().ok()?, Some(thread.parse().ok()?))),
        None => Some((key.parse().ok()?, None)),
    }
}

impl TelegramClient {
    async fn handle(&self, message: Message) {
        let Some(author) = message.from.as_deref() else {
            return;
        };
        if author.id == self.bot_user.id {
            return;
        }

        let bot_name = &self.bot_user.first_name;
        let attachments = self.collect_attachments(&message).await;
        let text = filter(&message, &self.bot_user, bot_name);
        if text.is_none() && attachments.is_empty() {
            return;
        }

        let name = full_name(author);
        let msg = format!("{}: {}", identity(&name, author.id), text.unwrap_or_default());
        let thread_id = message
            .is_topic_message
            .unwrap_or(false)
            .then_some(message.message_thread_id)
            .flatten();

        let constructor = ConversationConstructor {
            id: ConversationId(
                Platform::Telegram,
                conversation_key(message.chat.id, thread_id),
            ),
            is_group: message.chat.type_field != ChatType::Private,
            bot_identity: identity(bot_name, self.bot_user.id),
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
            let is_image = document
                .mime_type
                .as_deref()
                .is_some_and(|mime| mime.starts_with("image/"));
            attachments.push(match is_image {
                true => {
                    self.image_or_file(
                        &document.file_id,
                        document.file_size,
                        filename,
                        document.mime_type.clone().unwrap_or_default(),
                    )
                    .await
                }
                false => Attachment::File {
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
            false => self.download(file_id).await,
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

    async fn download(&self, file_id: &str) -> Result<Vec<u8>, String> {
        let file = self
            .bot
            .get_file(&GetFileParams::builder().file_id(file_id).build())
            .await
            .map_err(|err| describe(&err))?
            .result;
        let path = file.file_path.ok_or("Telegram returned no file path")?;
        let url = format!("https://api.telegram.org/file/bot{}/{path}", self.token);
        let response = self
            .bot
            .client
            .get(url)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|err| err.without_url().to_string())?;
        let bytes = response
            .bytes()
            .await
            .map_err(|err| err.without_url().to_string())?;
        Ok(bytes.to_vec())
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

fn filter(message: &Message, bot_user: &User, bot_name: &str) -> Option<String> {
    let (raw, entities) = match &message.text {
        Some(text) => (text.as_str(), message.entities.as_deref()),
        None => (
            message.caption.as_deref().unwrap_or_default(),
            message.caption_entities.as_deref(),
        ),
    };
    let mut text = raw.to_string();

    if let Some(username) = &bot_user.username {
        let mention = Regex::new(&format!(r"(?i)@{}\b", regex::escape(username)))
            .expect("escaped username is a valid regex");
        text = mention
            .replace_all(&text, regex::NoExpand(&identity(bot_name, bot_user.id)))
            .into_owned();
    }

    let units: Vec<u16> = raw.encode_utf16().collect();
    for entity in entities.unwrap_or_default() {
        let Some(user) = entity
            .user
            .as_ref()
            .filter(|_| entity.type_field == MessageEntityType::TextMention)
        else {
            continue;
        };
        let start = usize::from(entity.offset);
        let end = start + usize::from(entity.length);
        let Some(shown) = units.get(start..end).map(String::from_utf16_lossy) else {
            continue;
        };
        text = text.replacen(&shown, &identity(&full_name(user), user.id), 1);
    }

    let text = text.trim().trim_start_matches('/').trim().to_string();
    let space_trimmer = Regex::new(r"\s+").expect("static whitespace regex is valid");
    let text: String = space_trimmer.replace_all(&text, " ").trim().to_string();

    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: u64, first: &str, username: Option<&str>) -> User {
        serde_json::from_value(serde_json::json!({
            "id": id, "is_bot": username.is_some(), "first_name": first, "username": username,
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
    fn bot_mentions_and_commands_become_identities() {
        let bot = user(9, "A2", Some("a2bot"));
        let m = message(serde_json::json!({"text": "/ask@A2Bot   hey @a2bot, what's up?"}));
        assert_eq!(
            filter(&m, &bot, "A2").as_deref(),
            Some("ask(9) A2 hey (9) A2, what's up?")
        );
    }

    #[test]
    fn text_mentions_of_users_without_usernames_use_their_identity() {
        let bot = user(9, "A2", Some("a2bot"));
        let m = message(serde_json::json!({
            "text": "ask Zoë 🍷 about it",
            "entities": [{"type": "text_mention", "offset": 4, "length": 6,
                          "user": {"id": 5, "is_bot": false, "first_name": "Zoë", "last_name": "🍷"}}],
        }));
        assert_eq!(
            filter(&m, &bot, "A2").as_deref(),
            Some("ask (5) Zoë 🍷 about it")
        );
    }

    #[test]
    fn caption_is_used_when_there_is_no_text_and_blank_is_none() {
        let bot = user(9, "A2", None);
        let captioned = message(serde_json::json!({"caption": " look  at this "}));
        assert_eq!(filter(&captioned, &bot, "A2").as_deref(), Some("look at this"));
        let blank = message(serde_json::json!({"photo": []}));
        assert_eq!(filter(&blank, &bot, "A2"), None);
    }

    #[test]
    fn undecodable_batches_are_skipped_past_their_last_update() {
        let input = r#"{"ok":true,"result":[{"update_id":41,"x":{}},{"update_id":43,"y":1}]}"#;
        assert_eq!(last_update_id(input), Some(43));
        assert_eq!(last_update_id("not json"), None);
    }
}
