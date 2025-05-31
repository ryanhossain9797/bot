use std::{clone, sync::Arc};

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum UserChannel {
    Telegram,
    Discord,
}
impl UserChannel {
    fn to_string(&self) -> &'static str {
        match self {
            UserChannel::Telegram => "Telegram",
            UserChannel::Discord => "Discord",
        }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct UserId(pub UserChannel, pub String);

#[derive(Clone, Default)]
pub struct User {
    pub action_count: usize,
    pub maybe_poke_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub enum UserAction {
    NewMessage {
        msg: String,
        start_conversation: bool,
    },
    Poke,
    SendResult(Arc<anyhow::Result<()>>),
}
