
mod qwen;

use super::ParsedResponse;

pub(super) trait Parser: Send + Sync {
    /// `thinking`: whether the prompt opened a thinking block, so output before `close_marker`
    /// is reasoning; when it's off and no marker appears, the whole output is the answer.
    fn parse(&self, raw: &str, close_marker: &str, thinking: bool) -> ParsedResponse;
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
