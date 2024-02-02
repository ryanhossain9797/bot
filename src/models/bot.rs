use std::sync::Arc;

use serenity::{
    async_trait,
    http::Http as SerenityHttp,
    model::{
        channel::Message as DMessage,
        gateway::Ready,
        id::{ChannelId, UserId},
    },
    prelude::*,
};
use tokio::{
    io,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

pub enum BotAction {
    Ping {
        message: String,
    },
    HandleMessage {
        user_id: UserId,
        user_name: String,
        chat_id: ChannelId,
        http: Arc<SerenityHttp>,
        start_conversation: bool,
        msg: String,
    },
}

pub struct Bot {
    pub receiver: mpsc::Receiver<BotAction>,
}

#[derive(Clone)]
pub struct BotHandle {
    pub sender: mpsc::Sender<BotAction>,
}
