use std::{fmt::Display, sync::Arc};

use chrono::{DateTime, Utc};

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

#[derive(Clone, Debug, Default)]
pub enum UserState {
    #[default]
    Idle,
    RespondingToMessage,
    WaitingToSayGoodbye(Option<DateTime<Utc>>),
    SayingGoodbye,
}

#[derive(Clone, Default)]
pub struct User {
    pub state: UserState,
}

#[derive(Clone, Debug)]
pub enum UserAction {
    NewMessage {
        msg: String,
        start_conversation: bool,
    },
    Timeout,
    SendResult(Arc<anyhow::Result<()>>),
}
