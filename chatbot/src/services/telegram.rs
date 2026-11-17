use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use frankenstein::client_reqwest::Bot;
use frankenstein::methods::{
    GetChatMemberParams, GetFileParams, GetUpdatesParams, SendAnimationParams, SendDocumentParams,
    SendMessageParams, SendPhotoParams,
};
use frankenstein::types::{
    AllowedUpdate, ChatMember, ChatType, Message, MessageEntity, MessageEntityType, User,
};
use frankenstein::updates::UpdateContent;
use frankenstein::{AsyncTelegramApi, Error};
use regex::Regex;

use crate::{
    externals::bash_container_external::PulledFile,
    services::{identity, with_reply_marker},
    state_machines::conversation_state_machine::ConversationMachine,
    types::conversation::{ConversationAction, ConversationConstructor, ConversationId, Platform},
    types::media::{Attachment, Image, MessageImage},
};

const POLL_TIMEOUT_SECS: u32 = 30;
const ALBUM_POLL_TIMEOUT_SECS: u32 = 1;
const ALBUM_QUIET: Duration = Duration::from_secs(1);
const RETRY_DELAY: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_DOWNLOAD_BYTES: u64 = 20 * 1024 * 1024;
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(45);
pub const NO_PUBLIC_URL: &str = "none (Telegram attachment)";

static MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<@!?(\d+)>").expect("static mention regex is valid"));

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
        let mut waited = Duration::ZERO;
        let mut users: HashMap<u64, User> = HashMap::new();

        for chunk in chunks {
            let unknown: Vec<u64> = mentioned_ids(&chunk)
                .into_iter()
                .filter(|id| !users.contains_key(id))
                .collect();
            for id in unknown {
                if let Some(user) = self.member(chat_id, id).await {
                    users.insert(id, user);
                }
            }
            let (text, entities) = render_mentions(&chunk, &users);
            let params = SendMessageParams::builder()
                .chat_id(chat_id)
                .maybe_message_thread_id(thread_id)
                .text(text)
                .maybe_entities((!entities.is_empty()).then_some(entities))
                .build();
            with_rate_limit(&mut waited, || self.bot.send_message(&params)).await?;
        }

        for file in files {
            let dir = std::env::temp_dir().join(format!("a2-upload-{}", uuid::Uuid::new_v4().simple()));
            let sent = self
                .send_file(chat_id, thread_id, &dir, &file, &mut waited)
                .await;
            let _ = tokio::fs::remove_dir_all(&dir).await;
            sent?;
        }

        Ok(())
    }

    async fn member(&self, chat_id: i64, user_id: u64) -> Option<User> {
        let params = GetChatMemberParams::builder()
            .chat_id(chat_id)
            .user_id(user_id)
            .build();
        match self.bot.get_chat_member(&params).await {
            Ok(response) => Some(member_user(response.result)),
            Err(err) => {
                eprintln!("[telegram] can't mention {user_id}: {}", describe(&err));
                None
            }
        }
    }

    async fn send_file(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
        dir: &std::path::Path,
        file: &PulledFile,
        waited: &mut Duration,
    ) -> Result<(), String> {
        let staging = |err: std::io::Error| format!("staging {}: {err}", file.filename);
        let path = dir.join(&file.filename);
        tokio::fs::create_dir_all(dir).await.map_err(staging)?;
        tokio::fs::write(&path, &file.bytes).await.map_err(staging)?;

        let kind = upload_kind(&file.bytes);
        let shown = match kind {
            Upload::Photo => {
                let params = SendPhotoParams::builder()
                    .chat_id(chat_id)
                    .maybe_message_thread_id(thread_id)
                    .photo(path.clone())
                    .build();
                Some(with_rate_limit(waited, || self.bot.send_photo(&params)).await.map(drop))
            }
            Upload::Animation => {
                let params = SendAnimationParams::builder()
                    .chat_id(chat_id)
                    .maybe_message_thread_id(thread_id)
                    .animation(path.clone())
                    .build();
                Some(with_rate_limit(waited, || self.bot.send_animation(&params)).await.map(drop))
            }
            Upload::Document => None,
        };
        match shown {
            Some(Ok(())) => return Ok(()),
            Some(Err(err)) => eprintln!(
                "[telegram] sending {} as {kind:?} failed ({err}); sending it as a file",
                file.filename
            ),
            None => {}
        }

        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .maybe_message_thread_id(thread_id)
            .document(path)
            .build();
        with_rate_limit(waited, || self.bot.send_document(&params))
            .await
            .map(drop)
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

    let mut albums = Albums::default();

    loop {
        params.timeout = Some(match albums.is_empty() {
            true => POLL_TIMEOUT_SECS,
            false => ALBUM_POLL_TIMEOUT_SECS,
        });
        match poller.telegram.bot.get_updates(&params).await {
            Ok(response) => {
                for update in response.result {
                    params.offset = Some(i64::from(update.update_id) + 1);
                    if let UpdateContent::Message(message) = update.content {
                        for parts in albums.accept(*message, Instant::now()) {
                            poller.handle(parts).await;
                        }
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
        for parts in albums.take_quiet(Instant::now()) {
            poller.handle(parts).await;
        }
    }
}

struct Album {
    chat_id: i64,
    group_id: String,
    parts: Vec<Message>,
    last_part: Instant,
}

#[derive(Default)]
struct Albums(Vec<Album>);

impl Albums {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn accept(&mut self, message: Message, now: Instant) -> Vec<Vec<Message>> {
        let chat_id = message.chat.id;
        let Some(group_id) = message.media_group_id.clone() else {
            let mut ready = self.take_where(|album| album.chat_id == chat_id);
            ready.push(vec![message]);
            return ready;
        };
        match self
            .0
            .iter_mut()
            .find(|album| album.chat_id == chat_id && album.group_id == group_id)
        {
            Some(album) => {
                album.parts.push(message);
                album.last_part = now;
            }
            None => self.0.push(Album {
                chat_id,
                group_id,
                parts: vec![message],
                last_part: now,
            }),
        }
        Vec::new()
    }

    fn take_quiet(&mut self, now: Instant) -> Vec<Vec<Message>> {
        self.take_where(|album| now.duration_since(album.last_part) >= ALBUM_QUIET)
    }

    fn take_where(&mut self, ready: impl Fn(&Album) -> bool) -> Vec<Vec<Message>> {
        let (taken, waiting): (Vec<Album>, Vec<Album>) =
            std::mem::take(&mut self.0).into_iter().partition(ready);
        self.0 = waiting;
        taken
            .into_iter()
            .map(|mut album| {
                album.parts.sort_by_key(|part| part.message_id);
                album.parts
            })
            .collect()
    }
}

async fn with_rate_limit<T, Fut>(
    waited: &mut Duration,
    mut call: impl FnMut() -> Fut,
) -> Result<T, String>
where
    Fut: Future<Output = Result<T, Error>>,
{
    loop {
        let err = match call().await {
            Ok(value) => return Ok(value),
            Err(err) => err,
        };
        match retry_wait(&err, *waited) {
            Some(wait) => {
                eprintln!("[telegram] rate limited; retrying in {wait:?}");
                tokio::time::sleep(wait).await;
                *waited += wait;
            }
            None => return Err(describe(&err)),
        }
    }
}

fn retry_wait(err: &Error, waited: Duration) -> Option<Duration> {
    let Error::Api(response) = err else {
        return None;
    };
    let seconds = response.parameters.as_ref()?.retry_after?;
    let wait = Duration::from_secs(u64::from(seconds).max(1));
    (waited + wait <= MAX_RATE_LIMIT_WAIT).then_some(wait)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Upload {
    Photo,
    Animation,
    Document,
}

fn upload_kind(bytes: &[u8]) -> Upload {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP) => {
            Upload::Photo
        }
        Ok(image::ImageFormat::Gif) => Upload::Animation,
        _ => Upload::Document,
    }
}

fn member_user(member: ChatMember) -> User {
    match member {
        ChatMember::Creator(member) => member.user,
        ChatMember::Administrator(member) => member.user,
        ChatMember::Member(member) => member.user,
        ChatMember::Restricted(member) => member.user,
        ChatMember::Left(member) => member.user,
        ChatMember::Kicked(member) => member.user,
    }
}

fn mentioned_ids(text: &str) -> Vec<u64> {
    let mut ids: Vec<u64> = MENTION
        .captures_iter(text)
        .filter_map(|cap| cap[1].parse().ok())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn render_mentions(text: &str, users: &HashMap<u64, User>) -> (String, Vec<MessageEntity>) {
    let mut out = String::with_capacity(text.len());
    let mut entities = Vec::new();
    let mut last = 0;
    for cap in MENTION.captures_iter(text) {
        let whole = cap.get(0).expect("capture 0 is the whole match");
        out.push_str(&text[last..whole.start()]);
        last = whole.end();
        let user = cap[1].parse::<u64>().ok().and_then(|id| users.get(&id));
        let Some(user) = user else {
            out.push_str(&format!("({})", &cap[1]));
            continue;
        };
        let label = full_name(user);
        let offset = u16::try_from(out.encode_utf16().count());
        let length = u16::try_from(label.encode_utf16().count());
        out.push_str(&label);
        if let (Ok(offset), Ok(length)) = (offset, length) {
            entities.push(
                MessageEntity::builder()
                    .type_field(MessageEntityType::TextMention)
                    .offset(offset)
                    .length(length)
                    .user(user.clone())
                    .build(),
            );
        }
    }
    out.push_str(&text[last..]);
    (out, entities)
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
    async fn handle(&self, parts: Vec<Message>) {
        let Some(message) = parts.first() else {
            return;
        };
        let Some(author) = message.from.as_deref() else {
            return;
        };
        if author.id == self.bot_user.id {
            return;
        }

        let mut attachments = Vec::new();
        for part in &parts {
            attachments.extend(self.collect_attachments(part).await);
        }
        let text = parts.iter().find_map(|part| filter(part, &self.bot_user));
        if text.is_none() && attachments.is_empty() {
            return;
        }
        let replied_to = parts.iter().find_map(|part| self.replied_to(part));
        let text = with_reply_marker(text, replied_to);

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

    fn replied_to(&self, message: &Message) -> Option<String> {
        let replied = message.reply_to_message.as_deref()?;
        if message.is_topic_message == Some(true)
            && Some(replied.message_id) == message.message_thread_id
        {
            return None;
        }
        let author = replied.from.as_deref()?;
        let name = match author.id == self.bot_user.id {
            true => self.bot_user.first_name.clone(),
            false => full_name(author),
        };
        Some(identity(&name, author.id))
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

        attachments.extend(listed_media(message));
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

fn listed_media(message: &Message) -> Vec<Attachment> {
    let listed = |name: Option<&String>, mime: Option<&String>, unique: &str, kind: &str, default: (&str, &str)| {
        Attachment::File {
            filename: name.cloned().unwrap_or_else(|| format!("{kind}_{unique}.{}", default.1)),
            content_type: Some(mime.cloned().unwrap_or_else(|| default.0.to_string())),
            url: NO_PUBLIC_URL.to_string(),
        }
    };
    let mp4 = ("video/mp4", "mp4");
    [
        message.video.as_deref().map(|v| {
            listed(v.file_name.as_ref(), v.mime_type.as_ref(), &v.file_unique_id, "video", mp4)
        }),
        message.video_note.as_deref().map(|v| listed(None, None, &v.file_unique_id, "video_note", mp4)),
        message.audio.as_deref().map(|a| {
            listed(a.file_name.as_ref(), a.mime_type.as_ref(), &a.file_unique_id, "audio", ("audio/mpeg", "mp3"))
        }),
        message.voice.as_deref().map(|v| {
            listed(None, v.mime_type.as_ref(), &v.file_unique_id, "voice", ("audio/ogg", "ogg"))
        }),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn full_name(user: &User) -> String {
    match &user.last_name {
        Some(last) => format!("{} {last}", user.first_name),
        None => user.first_name.clone(),
    }
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

    fn api_error(body: serde_json::Value) -> Error {
        Error::Api(serde_json::from_value(body).unwrap())
    }

    #[test]
    fn rate_limits_are_retried_within_the_wait_budget() {
        let limited = api_error(serde_json::json!({
            "ok": false, "error_code": 429, "description": "Too Many Requests: retry after 7",
            "parameters": {"retry_after": 7},
        }));
        assert_eq!(retry_wait(&limited, Duration::ZERO), Some(Duration::from_secs(7)));
        assert_eq!(retry_wait(&limited, Duration::from_secs(38)), Some(Duration::from_secs(7)));
        assert_eq!(retry_wait(&limited, Duration::from_secs(39)), None);

        let other = api_error(serde_json::json!({
            "ok": false, "error_code": 400, "description": "Bad Request: chat not found",
        }));
        assert_eq!(retry_wait(&other, Duration::ZERO), None);
    }

    #[test]
    fn mentions_become_text_mentions_with_utf16_offsets() {
        let zoe: User = serde_json::from_value(serde_json::json!({
            "id": 5, "is_bot": false, "first_name": "Zoë", "last_name": "🍷",
        }))
        .unwrap();
        let users = HashMap::from([(5, zoe.clone())]);
        let text = "🍷 hi <@5>, ask <@!5> or <@7>";
        assert_eq!(mentioned_ids(text), vec![5, 7]);
        let (out, entities) = render_mentions(text, &users);
        assert_eq!(out, "🍷 hi Zoë 🍷, ask Zoë 🍷 or (7)");
        let spans: Vec<(u16, u16, u64)> = entities
            .iter()
            .map(|e| (e.offset, e.length, e.user.as_ref().unwrap().id))
            .collect();
        assert_eq!(spans, vec![(6, 6, 5), (18, 6, 5)]);
        assert!(entities.iter().all(|e| e.type_field == MessageEntityType::TextMention));
    }

    #[test]
    fn replies_name_who_was_replied_to_but_not_the_topic_root() {
        let poller = Poller { telegram: Telegram::new("0:test"), bot_user: bot() };
        let reply = message(serde_json::json!({
            "text": "and this?",
            "reply_to_message": {"message_id": 3, "date": 0, "chat": {"id": -100, "type": "supergroup"},
                                 "from": {"id": 9, "is_bot": true, "first_name": "A2"}},
        }));
        assert_eq!(poller.replied_to(&reply).as_deref(), Some("(9) A2"));
        let in_topic = message(serde_json::json!({
            "text": "hello", "is_topic_message": true, "message_thread_id": 3,
            "reply_to_message": {"message_id": 3, "date": 0, "chat": {"id": -100, "type": "supergroup"},
                                 "from": {"id": 4, "is_bot": false, "first_name": "Op"}},
        }));
        assert_eq!(poller.replied_to(&in_topic), None);
    }

    #[test]
    fn album_parts_merge_once_quiet_and_flush_before_the_next_message() {
        let part = |id: i32, group: Option<&str>, chat: i64| {
            let mut m = message(serde_json::json!({"message_id": id, "photo": []}));
            m.media_group_id = group.map(str::to_string);
            m.chat.id = chat;
            m
        };
        let ids = |batches: Vec<Vec<Message>>| -> Vec<Vec<i32>> {
            batches.iter().map(|b| b.iter().map(|m| m.message_id).collect()).collect()
        };
        let start = Instant::now();
        let mut albums = Albums::default();

        assert!(ids(albums.accept(part(11, Some("g"), 1), start)).is_empty());
        assert!(ids(albums.accept(part(10, Some("g"), 1), start)).is_empty());
        assert!(ids(albums.accept(part(20, Some("g"), 2), start)).is_empty());
        assert!(ids(albums.accept(part(30, None, 3), start)) == vec![vec![30]]);
        assert!(ids(albums.take_quiet(start + Duration::from_millis(500))).is_empty());

        let later = start + Duration::from_millis(700);
        assert!(ids(albums.accept(part(12, Some("g"), 1), later)).is_empty());
        assert_eq!(ids(albums.take_quiet(start + ALBUM_QUIET)), vec![vec![20]]);

        assert_eq!(ids(albums.accept(part(13, None, 1), later)), vec![vec![10, 11, 12], vec![13]]);
        assert!(albums.is_empty());
    }

    #[test]
    fn video_audio_voice_and_video_notes_are_listed_as_files() {
        let m = message(serde_json::json!({
            "caption": "lol look",
            "video": {"file_id": "v", "file_unique_id": "V1", "width": 1, "height": 1, "duration": 3,
                      "file_name": "clip.mov", "mime_type": "video/quicktime"},
            "voice": {"file_id": "o", "file_unique_id": "O1", "duration": 4},
        }));
        let listed: Vec<(String, Option<String>, String)> = listed_media(&m)
            .into_iter()
            .map(|a| match a {
                Attachment::File { filename, content_type, url } => (filename, content_type, url),
                Attachment::Image { .. } => panic!("expected files"),
            })
            .collect();
        assert_eq!(listed, vec![
            ("clip.mov".into(), Some("video/quicktime".into()), NO_PUBLIC_URL.into()),
            ("voice_O1.ogg".into(), Some("audio/ogg".into()), NO_PUBLIC_URL.into()),
        ]);

        let note = message(serde_json::json!({
            "video_note": {"file_id": "n", "file_unique_id": "N1", "length": 240, "duration": 5},
        }));
        assert!(matches!(&listed_media(&note)[..],
            [Attachment::File { filename, .. }] if filename == "video_note_N1.mp4"));
        assert!(listed_media(&message(serde_json::json!({"text": "hi"}))).is_empty());
    }

    #[test]
    fn images_upload_as_photos_gifs_as_animations_and_the_rest_as_documents() {
        let encoded = |format: image::ImageFormat| {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 4))
                .write_to(&mut out, format)
                .unwrap();
            out.into_inner()
        };
        assert_eq!(upload_kind(&encoded(image::ImageFormat::Png)), Upload::Photo);
        assert_eq!(upload_kind(&encoded(image::ImageFormat::Jpeg)), Upload::Photo);
        assert_eq!(upload_kind(&encoded(image::ImageFormat::Gif)), Upload::Animation);
        assert_eq!(upload_kind(&encoded(image::ImageFormat::Bmp)), Upload::Document);
        assert_eq!(upload_kind(b"%PDF-1.7 report"), Upload::Document);
    }

    #[test]
    fn undecodable_batches_are_skipped_past_their_last_update() {
        let input = r#"{"ok":true,"result":[{"update_id":41,"x":{}},{"update_id":43,"y":1}]}"#;
        assert_eq!(last_update_id(input), Some(43));
        assert_eq!(last_update_id("not json"), None);
    }
}
