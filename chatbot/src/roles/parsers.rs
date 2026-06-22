//! The family of response parsers. Each member decodes one model family's wire format (its
//! reasoning markers + tool-call grammar) into a `ParsedResponse`. A role selects one by name from
//! its pack manifest (`[format] parser = "…"`), so swapping to a model with a known format is a
//! manifest change, not a code change — and a genuinely new format is a new `Parser` impl plus a
//! line in `from_name`. This mirrors how llama.cpp / mistral.rs organize parsing: a closed set of
//! per-family parsers, picked by name.

mod qwen;

use super::ParsedResponse;

/// A response parser for one model family's wire format. Implementors are zero-sized and held as
/// statics, so resolving one is just picking a `&'static dyn Parser`. The reasoning `close_marker`
/// stays a manifest fact passed in at parse time (it's data, like a template); the impl owns the
/// *grammar* (which is code).
pub(super) trait Parser: Send + Sync {
    fn parse(&self, raw: &str, close_marker: &str) -> ParsedResponse;
}

static QWEN: qwen::QwenParser = qwen::QwenParser;

/// Resolve the parser named in a pack manifest to its static implementation, erroring on an unknown
/// name.
pub(super) fn from_name(name: &str) -> anyhow::Result<&'static dyn Parser> {
    match name {
        "qwen" => Ok(&QWEN),
        other => Err(anyhow::anyhow!(
            "unknown parser '{other}' in manifest [format]; known parsers: qwen"
        )),
    }
}
