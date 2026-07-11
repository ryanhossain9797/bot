
mod qwen;

use super::ParsedResponse;

pub(super) trait Parser: Send + Sync {
    fn parse(&self, raw: &str, close_marker: &str) -> ParsedResponse;
}

static QWEN: qwen::QwenParser = qwen::QwenParser;

pub(super) fn from_name(name: &str) -> anyhow::Result<&'static dyn Parser> {
    match name {
        "qwen" => Ok(&QWEN),
        other => Err(anyhow::anyhow!(
            "unknown parser '{other}' in manifest [format]; known parsers: qwen"
        )),
    }
}
