use std::sync::LazyLock;

use regex::Regex;

use crate::{
    state_machines::conversation_state_machine::ConversationMachine,
    types::conversation::{
        ConversationAction, ConversationConstructor, ConversationId, Identity, IncomingMessage,
    },
};

pub mod discord;
pub mod telegram;

static WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("static whitespace regex is valid"));

pub fn normalize_text(text: &str) -> Option<String> {
    let text = text.trim().trim_start_matches('/').trim();
    let text = WHITESPACE.replace_all(text, " ").trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub struct Delivery {
    pub conversation: ConversationId,
    pub is_group: bool,
    pub bot: Identity,
    pub message: IncomingMessage,
}

pub async fn deliver(delivery: Delivery) {
    let Delivery {
        conversation,
        is_group,
        bot,
        message,
    } = delivery;
    if message.author.id == bot.id || message.is_empty() {
        return;
    }
    let constructor = ConversationConstructor {
        id: conversation,
        is_group,
        bot_identity: bot.to_string(),
    };
    re_framework::handle::<ConversationMachine>()
        .act_maybe_construct(constructor, ConversationAction::NewMessage(message))
        .await;
}

#[cfg(test)]
mod tests {
    use super::normalize_text;

    #[test]
    fn text_is_trimmed_collapsed_and_loses_a_leading_slash() {
        assert_eq!(normalize_text("  /ask   about\n\nthis ").as_deref(), Some("ask about this"));
        assert_eq!(normalize_text(" / "), None);
        assert_eq!(normalize_text(""), None);
    }
}
