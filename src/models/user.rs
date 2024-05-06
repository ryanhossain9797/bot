use std::{clone, sync::Arc};

use tokio::sync::mpsc;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
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
}

#[derive(Clone)]
pub enum UserAction {
    NewMessage {
        msg: String,
        start_conversation: bool,
    },
    SendResult(Arc<anyhow::Result<()>>),
}

#[derive(Clone)]
pub struct UserHandle {
    pub sender: mpsc::Sender<UserAction>,
}
