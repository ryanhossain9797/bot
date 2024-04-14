use std::sync::Arc;

use tokio::{
    io,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::user::UserId;

pub enum BotAction {
    Ping { message: String },
}

pub struct Bot {
    pub receiver: mpsc::Receiver<BotAction>,
}

#[derive(Clone)]
pub struct BotHandle {
    pub sender: mpsc::Sender<BotAction>,
}
