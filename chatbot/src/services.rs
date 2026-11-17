pub mod discord;
pub mod telegram;

pub fn identity(name: &str, id: u64) -> String {
    format!("({id}) {name}")
}

pub fn with_reply_marker(text: Option<String>, replied_to: Option<String>) -> Option<String> {
    match (replied_to, text) {
        (Some(who), Some(text)) => Some(format!("[replying to {who}] {text}")),
        (Some(who), None) => Some(format!("[replying to {who}]")),
        (None, text) => text,
    }
}
