use tokio::sync::mpsc;

#[allow(dead_code)]
#[derive(Debug)]
pub enum BotAction {
    Ping { message: String },
}

pub struct Bot {
    pub receiver: mpsc::Receiver<BotAction>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct BotHandle {
    pub sender: mpsc::Sender<BotAction>,
}
