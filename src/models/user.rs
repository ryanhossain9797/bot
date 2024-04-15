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

#[derive(Clone)]
pub struct User {
    pub action_count: usize,
}

pub enum UserAction {
    NewMessage {
        msg: String,
        start_conversation: bool,
    },
    SendResult(anyhow::Result<()>),
}

#[derive(Clone)]
pub struct UserHandle {
    pub sender: mpsc::Sender<UserAction>,
}
