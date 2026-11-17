use std::sync::{Arc, LazyLock};

use crate::{
    externals::bash_container_external::{pull_file, PulledFile},
    services::discord,
    types::conversation::{ConversationAction, Platform, ConversationId},
    Env,
};
use regex::Regex;

static ATTACH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\[attach_(?:file|image):\s*(.*?)\s*\]\]").expect("ATTACH_RE is a valid regex")
});

fn extract_attach_paths(text: &str) -> (String, Vec<String>) {
    let mut paths: Vec<String> = Vec::new();
    for cap in ATTACH_RE.captures_iter(text) {
        let path = cap[1].trim().to_string();
        if !path.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    let cleaned = ATTACH_RE.replace_all(text, "");
    let collapsed = Regex::new(r"[ \t]{2,}")
        .expect("static whitespace regex is valid")
        .replace_all(cleaned.trim(), " ")
        .to_string();
    (collapsed, paths)
}

fn split_message(content: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = content;

    while remaining.chars().count() > limit {
        let hard = remaining
            .char_indices()
            .nth(limit)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len());
        let split_at = remaining[..hard].rfind('\n').map(|i| i + 1).unwrap_or(hard);

        let (chunk, rest) = remaining.split_at(split_at);
        if !chunk.trim().is_empty() {
            chunks.push(chunk.to_string());
        }
        remaining = rest;
    }

    if !remaining.trim().is_empty() {
        chunks.push(remaining.to_string());
    }
    chunks
}

fn compose(platform: &Platform, message: Option<String>, tool_names: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(m) = message {
        parts.push(m);
    }
    if !tool_names.is_empty() {
        let body = match tool_names {
            [one] => format!("using tool: {one}"),
            many => format!("using multiple tools: {}", many.join(", ")),
        };
        parts.push(platform.subtext(&body));
    }
    parts.join("\n")
}

pub struct OutboundMessage {
    pub message: Option<String>,
    pub tool_names: Vec<String>,
}

impl OutboundMessage {
    pub fn is_empty(&self) -> bool {
        self.message.is_none() && self.tool_names.is_empty()
    }
}

pub async fn send_message(
    env: Arc<Env>,
    conversation_id: ConversationId,
    outbound: OutboundMessage,
) -> ConversationAction {
    let ConversationId(platform, key) = &conversation_id;
    let text = compose(platform, outbound.message, &outbound.tool_names);
    let (mut text, attach_paths) = extract_attach_paths(&text);

    let mut files: Vec<PulledFile> = Vec::new();
    for path in &attach_paths {
        match pull_file(&conversation_id.to_string(), path).await {
            Ok(file) => files.push(file),
            Err(err) => {
                eprintln!("[send] attach '{path}' failed: {err}");
                let note = platform.subtext(&format!("couldn't attach {path}: {err}"));
                text = if text.is_empty() { note } else { format!("{text}\n{note}") };
            }
        }
    }

    let chunks = split_message(&text, platform.message_limit());
    let result = match platform {
        Platform::Discord => discord::send(&env.discord_http, key, chunks, files).await,
        Platform::Telegram => match &env.telegram {
            Some(telegram) => telegram.send(key, chunks, files).await,
            None => Err("Telegram is not configured".to_string()),
        },
    };
    if let Err(err) = &result {
        eprintln!("[send] {platform} send failed: {err}");
    }
    ConversationAction::MessageSent(result)
}

#[cfg(test)]
mod tests {
    use super::{extract_attach_paths, split_message};

    #[test]
    fn split_message_respects_the_limit_and_prefers_newlines() {
        let text = format!("{}\n{}", "a".repeat(6), "b".repeat(6));
        assert_eq!(split_message(&text, 10), vec!["aaaaaa\n", "bbbbbb"]);
        assert_eq!(split_message(&"é".repeat(5), 2), vec!["éé", "éé", "é"]);
        assert!(split_message("  \n ", 10).is_empty());
    }

    #[test]
    fn strips_marker_and_captures_path() {
        let (text, paths) =
            extract_attach_paths("Here's the report [[attach_file:/tmp/report.pdf]] enjoy");
        assert_eq!(text, "Here's the report enjoy");
        assert_eq!(paths, vec!["/tmp/report.pdf"]);
    }

    #[test]
    fn marker_only_message_becomes_empty_text() {
        let (text, paths) = extract_attach_paths("[[attach_image:chart.png]]");
        assert_eq!(text, "");
        assert_eq!(paths, vec!["chart.png"]);
    }

    #[test]
    fn file_and_image_markers_both_match_dedup_and_preserve_order() {
        let (text, paths) =
            extract_attach_paths("a [[attach_file:/x]] b [[attach_image:/y]] c [[attach_file:/x]]");
        assert_eq!(text, "a b c");
        assert_eq!(paths, vec!["/x", "/y"]);
    }

    #[test]
    fn tolerates_internal_whitespace_and_no_markers() {
        let (text, paths) = extract_attach_paths("[[attach_file:  /a b/c.txt  ]]");
        assert_eq!(paths, vec!["/a b/c.txt"]);
        assert!(text.is_empty());

        let (plain, none) = extract_attach_paths("just a normal message");
        assert_eq!(plain, "just a normal message");
        assert!(none.is_empty());
    }
}
