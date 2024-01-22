use tokio::{
    io,
    sync::{mpsc, oneshot},
};

pub enum BotAction {
    Ping { message: String },
}

pub struct Bot {
    pub receiver: mpsc::Receiver<BotAction>,
}

pub struct BotHandle {
    pub sender: mpsc::Sender<BotAction>,
    pub on_kill: oneshot::Receiver<()>,
}
