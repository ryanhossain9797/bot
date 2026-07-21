use serde::Deserialize;
use serde_json::{json, Map, Value};
use strum::IntoEnumIterator;

use crate::chat_format::{ToolDefFunction, ToolDefinition};
use crate::types::conversation::{ToolKind, ToolType};

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
}

#[derive(Debug, Deserialize)]
struct VisitUrlArgs {
    url: String,
}

#[derive(Debug, Deserialize)]
struct RunBashArgs {
    command: String,
}

#[derive(Debug, Deserialize)]
struct PathArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default, deserialize_with = "de_lenient_opt_usize")]
    offset: Option<usize>,
    #[serde(default, deserialize_with = "de_lenient_opt_usize")]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NumOrStr<T> {
    Num(T),
    Str(String),
}

fn de_lenient_number<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    match NumOrStr::<T>::deserialize(deserializer)? {
        NumOrStr::Num(n) => Ok(n),
        NumOrStr::Str(s) => s.trim().parse::<T>().map_err(serde::de::Error::custom),
    }
}

fn de_lenient_opt_usize<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<NumOrStr<usize>>::deserialize(deserializer)? {
        None => Ok(None),
        Some(NumOrStr::Num(n)) => Ok(Some(n)),
        Some(NumOrStr::Str(s)) => match s.trim() {
            "" => Ok(None),
            t => t.parse::<usize>().map(Some).map_err(serde::de::Error::custom),
        },
    }
}

#[derive(Debug, Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
}

#[derive(Debug, Deserialize)]
struct UseSkillArgs {
    #[serde(default)]
    skill: Option<String>,
}

fn normalize_skill_name(raw: &str) -> Option<String> {
    let unquoted = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .trim();
    let base = unquoted.strip_suffix(".md").unwrap_or(unquoted).trim();
    (!base.is_empty()).then(|| base.to_string())
}

fn parse_use_skill(arguments: &str) -> Option<String> {
    let raw = arguments.trim();
    if raw.is_empty() {
        return None;
    }
    match serde_json::from_str::<UseSkillArgs>(raw) {
        Ok(args) => args.skill.as_deref().and_then(normalize_skill_name),
        Err(_) => normalize_skill_name(raw),
    }
}

#[derive(Debug, Deserialize)]
struct SetReminderArgs {
    #[serde(deserialize_with = "de_lenient_number")]
    delay_seconds: i64,
    note: String,
    addressee: String,
}

fn parse_args<T: serde::de::DeserializeOwned>(name: &str, arguments: &str) -> anyhow::Result<T> {
    serde_json::from_str(arguments)
        .map_err(|e| anyhow::anyhow!("{name} arguments failed to bind: {e} — raw: {arguments}"))
}

impl ToolKind {
    fn wire_name(&self) -> &'static str {
        match self {
            ToolKind::WebSearch => "web_search",
            ToolKind::VisitUrl => "visit_url",
            ToolKind::RunBashCommand => "run_bash_command",
            ToolKind::ResetBashContainer => "reset_bash_container",
            ToolKind::ViewImage => "view_image",
            ToolKind::ReadFile => "read_file",
            ToolKind::EditFile => "edit_file",
            ToolKind::UseSkill => "use_skill",
            ToolKind::SetReminder => "set_reminder",
            ToolKind::MetaNoOpExtraTurn => "meta_no_op_extra_turn",
            ToolKind::MetaMalformed => "meta_malformed_tool_call",
        }
    }

    fn announcement(&self) -> Option<&'static str> {
        match self {
            ToolKind::MetaNoOpExtraTurn | ToolKind::MetaMalformed => None,
            _ => Some(self.wire_name()),
        }
    }

    fn definition(&self) -> Option<ToolDefinition> {
        let (description, parameters): (&'static str, Value) = match self {
            ToolKind::WebSearch => (
                "Search the web — ONE focused topic per query; search one fact at a time, never pile attributes into a single query. Snippets only, usually not enough for specifics (dates, numbers, names, quotes) — open the best result with visit_url and read it before answering. For several facts, fire several single-topic searches in the same turn (parallel is fine).",
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "A few keywords for a single focused question, e.g. \"rust async runtime comparison\" or \"Stark Frieren hair color\". Don't pile unrelated attributes into one query — search one fact at a time." }
                    },
                    "required": ["query"]
                }),
            ),
            ToolKind::VisitUrl => (
                "Read a web page in full (its readable text). The normal next step after web_search — open the best result and read it before answering anything detailed or factual; the snippet alone is rarely enough. Also works on a URL the user gave you.",
                json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "The full URL to fetch, e.g. \"https://example.com/article\" (typically taken from a web_search result)" }
                    },
                    "required": ["url"]
                }),
            ),
            ToolKind::RunBashCommand => (
                "Run a bash command in your own private Linux sandbox (persistent across calls within this conversation; has python3, pip, curl, git, and internet access). Use it to compute, write and run scripts, fetch and process data, install packages — anything a shell can do. The filesystem and installed packages persist between calls, so you can build up state. Not connected to the user's machine.",
                json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The bash command to run, e.g. \"python3 -c 'print(2**10)'\" or \"pip install requests && python3 script.py\". Multi-line scripts are fine." }
                    },
                    "required": ["command"]
                }),
            ),
            ToolKind::ResetBashContainer => (
                "Wipe your sandbox and start fresh — destroys the current Linux environment (files, installed packages, processes) and the next run_bash_command boots a clean one. Use if it's in a broken state or you want a clean slate.",
                json!({ "type": "object", "properties": {}, "required": [] }),
            ),
            ToolKind::ViewImage => (
                "Privately inspect an image yourself — the user does NOT see it. To show an image to the user, don't use this tool: write the marker `[[attach_image:PATH]]` in your reply instead. The path points to a file in the SAME private Linux environment as run_bash_command — create, download, or generate the image there first (e.g. with matplotlib, imagemagick, or curl). The file must be a valid image (PNG, JPEG, GIF, or WebP). Use it to inspect plots, screenshots, or downloaded images before deciding what to do next.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the image file inside your bash sandbox, e.g. \"/tmp/plot.png\" or \"chart.png\" (relative to the sandbox working directory)." }
                    },
                    "required": ["path"]
                }),
            ),
            ToolKind::ReadFile => (
                "Read a text file from your bash sandbox — the SAME private Linux environment as run_bash_command. Returns the file's contents with line numbers. Reads the whole file by default; for a large file, pass offset and/or limit to read a slice.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the file inside your bash sandbox, e.g. \"/home/user/app/main.py\" or \"notes.txt\" (relative to the sandbox working directory)." },
                        "offset": { "type": "integer", "description": "1-based line number to start reading from. Defaults to the first line." },
                        "limit": { "type": "integer", "description": "Maximum number of lines to read, starting at offset. Defaults to the rest of the file." }
                    },
                    "required": ["path"]
                }),
            ),
            ToolKind::EditFile => (
                "Edit a text file in your bash sandbox by replacing an exact string. `old_string` must appear EXACTLY ONCE in the current file — include enough surrounding context to make it unique — and is replaced with `new_string`. Returns a diff of the change.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the file inside your bash sandbox (the same one you read_file'd)." },
                        "old_string": { "type": "string", "description": "The exact text to replace, copied from read_file output WITHOUT the line-number prefixes. Must match the file exactly (whitespace included) and be unique." },
                        "new_string": { "type": "string", "description": "The text to put in its place. Use an empty string to delete the matched text." }
                    },
                    "required": ["path", "old_string", "new_string"]
                }),
            ),
            ToolKind::UseSkill => (
                "Consult your skill library — short reference guides for specific tasks. Available skills: `document_conversion` (converting between document/ebook formats — MOBI, EPUB, PDF, DOCX and more, with the right tool for each). Call it with a skill's name to read that skill in full and follow it, or with NO arguments to list every skill. Use the relevant skill before attempting a task it covers.",
                json!({
                    "type": "object",
                    "properties": {
                        "skill": { "type": "string", "description": "The name of the skill to read, exactly as shown in the no-argument listing, e.g. \"document_conversion\". The .md extension, surrounding quotes, and extra spaces are all optional. Omit this field entirely to list all available skills instead of reading one." }
                    },
                    "required": []
                }),
            ),
            ToolKind::SetReminder => (
                "Set a reminder to message a user in the future. When the delay elapses, the conversation wakes on its own and you are prompted to send the reminder — so you do NOT keep this turn open waiting; call it, tell the user you've set it, and finish. Compute delay_seconds yourself from the current time in the metadata footer. Note: reminders are lost on a redeploy (they do survive a normal restart).",
                json!({
                    "type": "object",
                    "properties": {
                        "delay_seconds": { "type": "integer", "description": "How far in the future to fire, in seconds from now, e.g. 7200 for two hours. Compute it from the current time shown in the footer." },
                        "note": { "type": "string", "description": "What to remind the user about, in your own words — this is handed back to you when the reminder fires, e.g. \"take your meds\" or \"the meeting with Alex starts in 10 minutes\"." },
                        "addressee": { "type": "string", "description": "Who the reminder is for — the display name of the user it should be delivered to, as shown in the \"(id) Name:\" tag. In a one-to-one chat this is that user; in a group, whoever asked for it." }
                    },
                    "required": ["delay_seconds", "note", "addressee"]
                }),
            ),
            ToolKind::MetaNoOpExtraTurn => (
                "Give yourself another turn after this reply — use it to say something to the user across multiple steps. Put the first part in this reply's message, call this tool, and you'll be prompted again to continue with the next part. Pointless to call with no message (nothing is sent to the user), and pointless to call alongside other tools (a tool call already earns you another turn).",
                json!({ "type": "object", "properties": {}, "required": [] }),
            ),
            ToolKind::MetaMalformed => return None,
        };
        Some(ToolDefinition {
            kind: "function",
            function: ToolDefFunction {
                name: self.wire_name(),
                description,
                parameters,
            },
        })
    }
}

impl ToolType {
    pub fn tool_definitions() -> Vec<ToolDefinition> {
        ToolKind::iter().filter_map(|k| k.definition()).collect()
    }

    pub fn wire_name(&self) -> &'static str {
        ToolKind::from(self).wire_name()
    }

    pub fn announcement(&self) -> Option<&'static str> {
        ToolKind::from(self).announcement()
    }

    pub fn arguments_map(&self) -> Map<String, Value> {
        match self {
            ToolType::WebSearch { query } => {
                [("query".to_string(), json!(query))].into_iter().collect()
            }
            ToolType::VisitUrl { url } => [("url".to_string(), json!(url))].into_iter().collect(),
            ToolType::RunBashCommand { command } => [("command".to_string(), json!(command))]
                .into_iter()
                .collect(),
            ToolType::ResetBashContainer => Map::new(),
            ToolType::ViewImage { path } => {
                [("path".to_string(), json!(path))].into_iter().collect()
            }
            ToolType::ReadFile {
                path,
                offset,
                limit,
            } => [
                Some(("path".to_string(), json!(path))),
                offset.as_ref().map(|n| ("offset".to_string(), json!(n))),
                limit.as_ref().map(|n| ("limit".to_string(), json!(n))),
            ]
            .into_iter()
            .flatten()
            .collect(),
            ToolType::EditFile {
                path,
                old_string,
                new_string,
            } => [
                ("path".to_string(), json!(path)),
                ("old_string".to_string(), json!(old_string)),
                ("new_string".to_string(), json!(new_string)),
            ]
            .into_iter()
            .collect(),
            ToolType::UseSkill { skill } => skill
                .as_ref()
                .map(|s| ("skill".to_string(), json!(s)))
                .into_iter()
                .collect(),
            ToolType::SetReminder {
                delay_seconds,
                note,
                addressee,
            } => [
                ("delay_seconds".to_string(), json!(delay_seconds)),
                ("note".to_string(), json!(note)),
                ("addressee".to_string(), json!(addressee)),
            ]
            .into_iter()
            .collect(),
            ToolType::MetaNoOpExtraTurn => Map::new(),
            ToolType::MetaMalformed { .. } => Map::new(),
        }
    }

    pub fn bind(name: &str, arguments: &str) -> anyhow::Result<ToolType> {
        let kind = ToolKind::iter()
            .find(|k| k.wire_name() == name)
            .ok_or_else(|| anyhow::anyhow!("model called an unknown tool: {name}"))?;

        match kind {
            ToolKind::WebSearch => Ok(ToolType::WebSearch {
                query: parse_args::<WebSearchArgs>(name, arguments)?.query,
            }),
            ToolKind::VisitUrl => Ok(ToolType::VisitUrl {
                url: parse_args::<VisitUrlArgs>(name, arguments)?.url,
            }),
            ToolKind::RunBashCommand => Ok(ToolType::RunBashCommand {
                command: parse_args::<RunBashArgs>(name, arguments)?.command,
            }),
            ToolKind::ResetBashContainer => Ok(ToolType::ResetBashContainer),
            ToolKind::ViewImage => Ok(ToolType::ViewImage {
                path: parse_args::<PathArgs>(name, arguments)?.path,
            }),
            ToolKind::ReadFile => {
                let args = parse_args::<ReadFileArgs>(name, arguments)?;
                Ok(ToolType::ReadFile {
                    path: args.path,
                    offset: args.offset,
                    limit: args.limit,
                })
            }
            ToolKind::EditFile => {
                let args = parse_args::<EditFileArgs>(name, arguments)?;
                Ok(ToolType::EditFile {
                    path: args.path,
                    old_string: args.old_string,
                    new_string: args.new_string,
                })
            }
            ToolKind::UseSkill => Ok(ToolType::UseSkill {
                skill: parse_use_skill(arguments),
            }),
            ToolKind::SetReminder => {
                let args = parse_args::<SetReminderArgs>(name, arguments)?;
                Ok(ToolType::SetReminder {
                    delay_seconds: args.delay_seconds,
                    note: args.note,
                    addressee: args.addressee,
                })
            }
            ToolKind::MetaNoOpExtraTurn => Ok(ToolType::MetaNoOpExtraTurn),
            ToolKind::MetaMalformed => Ok(ToolType::MetaMalformed {
                report: "meta_malformed_tool_call is internal and cannot be called directly."
                    .to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_malformed_is_internal_not_advertised() {
        let names: Vec<&str> = ToolType::tool_definitions()
            .iter()
            .map(|d| d.function.name)
            .collect();
        assert!(
            !names.contains(&"meta_malformed_tool_call"),
            "internal tool must not be advertised to the model"
        );
        assert!(names.contains(&"set_reminder"));
        assert_eq!(ToolType::MetaMalformed { report: String::new() }.announcement(), None);
        assert!(
            ToolType::bind("attach_file", r#"{"path":"/x"}"#).is_err(),
            "unknown tool must error at bind (the llm layer turns this into MetaMalformed)"
        );
    }

    #[test]
    fn read_file_accepts_stringified_numbers() {
        let bound = ToolType::bind(
            "read_file",
            r#"{"path":"/tmp/x.json","offset":"60","limit":"20"}"#,
        )
        .expect("stringified numbers should bind");
        assert!(matches!(
            bound,
            ToolType::ReadFile { offset: Some(60), limit: Some(20), .. }
        ));
    }

    #[test]
    fn read_file_accepts_numeric_and_absent() {
        let numeric = ToolType::bind("read_file", r#"{"path":"/x","offset":5}"#).unwrap();
        assert!(matches!(
            numeric,
            ToolType::ReadFile { offset: Some(5), limit: None, .. }
        ));

        let absent = ToolType::bind("read_file", r#"{"path":"/x"}"#).unwrap();
        assert!(matches!(
            absent,
            ToolType::ReadFile { offset: None, limit: None, .. }
        ));
    }

    #[test]
    fn use_skill_lists_when_no_argument() {
        for args in ["", "{}", r#"{"skill":""}"#, r#"{"skill":"   "}"#, r#"{"skill":null}"#] {
            assert!(
                matches!(ToolType::bind("use_skill", args), Ok(ToolType::UseSkill { skill: None })),
                "args {args:?} should list (skill: None)"
            );
        }
    }

    #[test]
    fn use_skill_parses_leniently() {
        let expected = "document_conversion";
        for raw in [
            "document_conversion",
            "document_conversion.md",
            "  document_conversion  ",
            "\"document_conversion\"",
            "'document_conversion.md'",
            "  \"document_conversion.md\" ",
        ] {
            let args = serde_json::json!({ "skill": raw }).to_string();
            match ToolType::bind("use_skill", &args) {
                Ok(ToolType::UseSkill { skill: Some(name) }) => {
                    assert_eq!(name, expected, "raw {raw:?} should normalize to {expected}")
                }
                other => panic!("raw {raw:?} did not parse: {other:?}"),
            }
        }
    }

    #[test]
    fn set_reminder_accepts_numeric_and_stringified_delay() {
        let numeric = ToolType::bind(
            "set_reminder",
            r#"{"delay_seconds":7200,"note":"take meds","addressee":"Alice"}"#,
        )
        .unwrap();
        assert!(matches!(
            numeric,
            ToolType::SetReminder { delay_seconds: 7200, .. }
        ));

        let stringified = ToolType::bind(
            "set_reminder",
            r#"{"delay_seconds":"7200","note":"take meds","addressee":"Alice"}"#,
        )
        .unwrap();
        assert!(matches!(
            stringified,
            ToolType::SetReminder { delay_seconds: 7200, .. }
        ));
    }
}
