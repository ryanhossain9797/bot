use crate::{
    configuration::client_tokens::{BRAVE_SEARCH_TOKEN, SEARXNG_URL},
    externals::bash_container_external::{
        clip_to, pull_image, read_file, reset_bash, run_bash, write_file, ACTUAL_MAX, SIMPLIFIED_MAX,
    },
    types::conversation::{
        ToolCall, ToolResultData, ToolType, ConversationAction,
        MAX_SEARCH_DESCRIPTION_LENGTH,
    },
};
use rs_trafilatura::extract;
use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

#[derive(Deserialize)]
struct SearxngResponse {
    query: String,
    #[serde(default)]
    answers: Vec<SearxngAnswer>,
    #[serde(default)]
    infoboxes: Vec<SearxngInfobox>,
    results: Vec<SearxngResult>,
}

#[derive(Deserialize)]
struct SearxngAnswer {
    #[serde(default)]
    answer: Option<String>,
}

#[derive(Deserialize)]
struct SearxngInfobox {
    #[serde(default)]
    infobox: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct SearxngResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
}

const MAX_SEARCH_RESULTS: usize = 8;
const SIMPLIFIED_SEARCH_RESULTS: usize = 3;

async fn fetch_web_search(query: &str) -> anyhow::Result<ToolResultData> {
    if let Err(e) = crate::externals::searxng_external::ensure_searxng().await {
        eprintln!("[searxng] ensure failed (querying anyway): {e}");
    }

    let search_url = format!(
        "{}/search?q={}&format=json",
        SEARXNG_URL.trim_end_matches('/'),
        urlencoding::encode(query)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let response = client
        .get(&search_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to SearxNG at {SEARXNG_URL}: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow::anyhow!(
            "SearxNG returned error status {status}: {error_text}"
        ));
    }

    let search_response = response
        .json::<SearxngResponse>()
        .await
        .map_err(|e| {
            anyhow::anyhow!("Failed to parse SearxNG response: {e}. Ensure SEARXNG_URL points at a SearxNG instance with JSON format enabled (search.formats includes json).")
        })?;

    let original_query = search_response.query;

    let mut lead = String::new();
    for answer in search_response
        .answers
        .iter()
        .filter_map(|a| a.answer.as_deref())
        .filter(|s| !s.is_empty())
    {
        lead.push_str(&format!("Instant answer: {answer}\n\n"));
    }
    if let Some(infobox) = search_response.infoboxes.first() {
        if let Some(content) = infobox.content.as_deref().filter(|s| !s.is_empty()) {
            let title = infobox.infobox.as_deref().unwrap_or("Info");
            lead.push_str(&format!("{title}: {content}\n\n"));
        }
    }

    let formatted_results: Vec<String> = search_response
        .results
        .into_iter()
        .take(MAX_SEARCH_RESULTS)
        .map(|result| {
            let title = result.title.as_deref().unwrap_or("null");
            let url = result.url.as_deref().unwrap_or("null");
            let description = result.content.as_deref().unwrap_or("null");

            let safe_len = description.floor_char_boundary(MAX_SEARCH_DESCRIPTION_LENGTH);
            let description = &description[..safe_len];
            let published = result
                .published_date
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|d| format!("Published: {d}\n"))
                .unwrap_or_default();
            format!("Title: {title}\nURL to visit: {url}\n{published}Description: {description}\n\n")
        })
        .collect();

    let split = SIMPLIFIED_SEARCH_RESULTS.min(formatted_results.len());
    let (primary, secondary) = formatted_results.split_at(split);

    let simplified = format!(
        "{lead}Search results for \"{original_query}\":\n{}",
        primary.join("\n")
    );

    let actual = format!("{simplified}\n{}", secondary.join("\n"));

    Ok(ToolResultData::text(actual, simplified))
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct BraveSearchResponse {
    query: BraveSearchQuery,
    web: BraveWebResults,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct BraveSearchQuery {
    original: String,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct BraveWebResults {
    results: Vec<BraveSearchResult>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct BraveSearchResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[allow(dead_code)]
async fn fetch_web_search_brave(query: &str) -> anyhow::Result<ToolResultData> {
    let search_url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}",
        urlencoding::encode(query)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let response = client
        .get(&search_url)
        .header("Accept", "application/json")
        .header("X-Subscription-Token", BRAVE_SEARCH_TOKEN)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to Brave Search API: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(anyhow::anyhow!(
            "Brave Search API returned error status {}: {}",
            status,
            error_text
        ));
    }

    let search_response = response
        .json::<BraveSearchResponse>()
        .await
        .map_err(|e| {
            anyhow::anyhow!("Failed to parse Brave Search response: {}. Make sure BRAVE_SEARCH_TOKEN is set correctly.", e)
        })?;

    let original_query = search_response.query.original;
    let formatted_results: Vec<String> = search_response
        .web
        .results
        .into_iter()
        .take(MAX_SEARCH_RESULTS)
        .map(|result| {
            let title = result.title.as_deref().unwrap_or("null");
            let url = result.url.as_deref().unwrap_or("null");
            let description = result.description.as_deref().unwrap_or("null");

            let safe_len = description.floor_char_boundary(MAX_SEARCH_DESCRIPTION_LENGTH);
            let description = &description[..safe_len];
            format!("Title: {title}\nURL to visit: {url}\nDescription: {description}\n\n")
        })
        .collect();

    let split = SIMPLIFIED_SEARCH_RESULTS.min(formatted_results.len());
    let (primary, secondary) = formatted_results.split_at(split);

    let simplified = format!(
        "Search results for \"{original_query}\":\n{}",
        primary.join("\n")
    );

    let actual = format!("{simplified}\n{}", secondary.join("\n"));

    Ok(ToolResultData::text(actual, simplified))
}

#[allow(dead_code)]
#[derive(Debug)]
struct ExtractedPage {
    final_url: String,
    content: String,
}

async fn fetch_page(url: &str) -> anyhow::Result<ExtractedPage> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch URL: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("HTTP error {}", status));
    }

    let final_url = response.url().to_string();

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !content_type.to_lowercase().contains("text/html") {
        return Err(anyhow::anyhow!("URL is not HTML"));
    }

    let html_body = response
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?;

    let content = extract(&html_body)?.content_text;

    Ok(ExtractedPage { final_url, content })
}

async fn fetch_url_content(url: &str) -> anyhow::Result<ToolResultData> {
    pub const MAX_ACTUAL_WEB_CONTENT_LENGTH: usize = 50000;
    pub const MAX_SIMPLIFIED_WEB_CONTENT_LENGTH: usize = 2000;

    let extracted = fetch_page(url).await?;

    let actual_end = extracted.content.floor_char_boundary(MAX_ACTUAL_WEB_CONTENT_LENGTH);
    let actual_content = &extracted.content[..actual_end];

    let simplified_end = extracted
        .content
        .floor_char_boundary(MAX_SIMPLIFIED_WEB_CONTENT_LENGTH);
    let simplified_content = &extracted.content[..simplified_end];

    let actual = format!("Content of {url}:\n{actual_content}");
    let simplified = format!("Content of {url}:\n{simplified_content}");

    Ok(ToolResultData::text(actual, simplified))
}

fn content_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn number_lines(content: &str, from: usize, count: usize) -> String {
    content
        .lines()
        .enumerate()
        .skip(from.saturating_sub(1))
        .take(count)
        .map(|(i, line)| format!("{}\t{}", i + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_diff(content: &str, old: &str, new: &str) -> String {
    const CONTEXT_LINES: usize = 3;

    let Some(start) = content.find(old) else {
        return old
            .lines()
            .map(|l| format!("- {l}"))
            .chain(new.lines().map(|l| format!("+ {l}")))
            .collect::<Vec<_>>()
            .join("\n");
    };
    let end = start + old.len();

    let line_start = content[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = content[end..].find('\n').map_or(content.len(), |off| end + off);

    let old_block = &content[line_start..line_end];
    let new_block = format!("{}{new}{}", &content[line_start..start], &content[end..line_end]);

    let before: Vec<&str> = content[..line_start].lines().collect();
    let before_ctx = &before[before.len().saturating_sub(CONTEXT_LINES)..];
    let after = content[line_end..].strip_prefix('\n').unwrap_or("");
    let after_ctx = after.lines().take(CONTEXT_LINES);

    before_ctx
        .iter()
        .map(|l| format!("  {l}"))
        .chain(old_block.lines().map(|l| format!("- {l}")))
        .chain(new_block.lines().map(|l| format!("+ {l}")))
        .chain(after_ctx.map(|l| format!("  {l}")))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn run_tool(
    conversation_id: &str,
    tool_type: ToolType,
    expected_file_hash: Option<String>,
) -> Result<ToolResultData, String> {
    match tool_type {
        ToolType::MetaNoOpExtraTurn => {
            let note = "You have another turn — continue with the next part of your message now.";
            Ok(ToolResultData::text(note.to_string(), note.to_string()))
        }
        ToolType::WebSearch { query } => fetch_web_search(&query).await.map_err(|e| e.to_string()),
        ToolType::VisitUrl { url } => fetch_url_content(&url).await.map_err(|e| e.to_string()),
        ToolType::RunBashCommand { command } => run_bash(conversation_id, &command).await,
        ToolType::ResetBashContainer => reset_bash(conversation_id).await,
        ToolType::ViewImage { path } => {
            let image = pull_image(conversation_id, &path).await?;
            let note = format!("Image '{path}' loaded — shown below. This is for your eyes only: the user has NOT seen this image.");
            Ok(ToolResultData {
                actual: note.clone(),
                simplified: note,
                image_for_assistant: Some(image),
                metadata: HashMap::new(),
            })
        }
        ToolType::ReadFile { path, offset, limit } => {
            let content = read_file(conversation_id, &path).await?;
            let hash = content_hash(&content);
            let from = offset.unwrap_or(1).max(1);
            let count = limit.unwrap_or(usize::MAX);
            let numbered = number_lines(&content, from, count);
            let body = if numbered.is_empty() {
                format!("(file '{path}' is empty, or the requested line range has no lines)")
            } else {
                clip_to(&numbered, ACTUAL_MAX)
            };
            Ok(ToolResultData {
                simplified: clip_to(&body, SIMPLIFIED_MAX),
                actual: body,
                image_for_assistant: None,
                metadata: HashMap::from([("file_hash".to_string(), hash)]),
            })
        }
        ToolType::EditFile { path, old_string, new_string } => {
            let content = read_file(conversation_id, &path).await?;
            let live_hash = content_hash(&content);
            match expected_file_hash {
                None => return Err(format!(
                    "'{path}' hasn't been read yet — call read_file on it before editing."
                )),
                Some(h) if h != live_hash => return Err(format!(
                    "'{path}' has changed since you last read it — call read_file on it again before editing."
                )),
                Some(_) => {}
            }
            match content.matches(&old_string).count() {
                0 => return Err(format!(
                    "old_string was not found in '{path}'. Copy it exactly from read_file output (without the line-number prefixes)."
                )),
                1 => {}
                n => return Err(format!(
                    "old_string appears {n} times in '{path}' — include more surrounding context so it matches exactly once."
                )),
            }
            let updated = content.replacen(&old_string, &new_string, 1);
            write_file(conversation_id, &path, &updated).await?;
            let body = format!("Edited '{path}':\n{}", render_diff(&content, &old_string, &new_string));
            Ok(ToolResultData {
                simplified: clip_to(&body, SIMPLIFIED_MAX),
                actual: body,
                image_for_assistant: None,
                metadata: HashMap::from([("file_hash".to_string(), content_hash(&updated))]),
            })
        }
        ToolType::UseSkill { skill } => match skill {
            None => {
                let list = crate::externals::skill_external::list_skills().await?;
                Ok(ToolResultData::text(list.clone(), list))
            }
            Some(name) => {
                let content = crate::externals::skill_external::read_skill(&name).await?;
                let body = clip_to(&content, ACTUAL_MAX);
                let simplified = clip_to(&body, SIMPLIFIED_MAX);
                Ok(ToolResultData::text(body, simplified))
            }
        },
        ToolType::SetReminder { .. } => Err(
            "set_reminder is handled by the conversation runtime, not the tool executor".to_string(),
        ),
        ToolType::MetaMalformed { .. } => Err(
            "meta_malformed_tool_call is handled by the conversation runtime, not the tool executor"
                .to_string(),
        ),
    }
}

pub async fn execute_tool(
    conversation_id: String,
    tool_call: ToolCall,
    expected_file_hash: Option<String>,
) -> ConversationAction {
    let tool_name = tool_call.tool_type.wire_name().to_string();
    let result = run_tool(&conversation_id, tool_call.tool_type, expected_file_hash).await;
    if let Err(err) = &result {
        eprintln!("[tool {tool_name} id {}] failed: {err}", tool_call.id);
    }
    ConversationAction::ToolResult {
        id: tool_call.id,
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_web_search() {
        let search_results = fetch_web_search("Rust programming").await.unwrap();
        assert!(search_results.actual.contains("Search results for"));
        assert!(search_results.actual.contains("Rust programming"));
    }

    #[tokio::test]
    async fn test_fetch_url_content_real() {
        let content = fetch_url_content("https://example.com").await.unwrap();
        assert!(content.actual.contains("Example Domain"));
        assert!(content
            .actual
            .contains("This domain is for use in documentation examples"));
        assert!(content.actual.contains("https://iana.org/domains/example"));
    }
}
