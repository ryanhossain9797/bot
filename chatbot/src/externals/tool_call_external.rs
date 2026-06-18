use crate::{
    configuration::client_tokens::{BRAVE_SEARCH_TOKEN, SEARXNG_URL},
    externals::bash_container_external::{reset_bash, run_bash},
    types::conversation::{
        ToolCall, ToolResultData, ToolType, ConversationAction,
        MAX_SEARCH_DESCRIPTION_LENGTH,
    },
};
use rs_trafilatura::extract;
use serde::Deserialize;

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

    Ok(ToolResultData { actual, simplified })
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

    Ok(ToolResultData { actual, simplified })
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

    Ok(ToolResultData { actual, simplified })
}

async fn run_tool(conversation_id: &str, tool_type: ToolType) -> Result<ToolResultData, String> {
    match tool_type {
        ToolType::WebSearch { query } => fetch_web_search(&query).await.map_err(|e| e.to_string()),
        ToolType::VisitUrl { url } => fetch_url_content(&url).await.map_err(|e| e.to_string()),
        ToolType::RunBashCommand { command } => run_bash(conversation_id, &command).await,
        ToolType::ResetBashContainer => reset_bash(conversation_id).await,
    }
}

pub async fn execute_tool(conversation_id: String, tool_call: ToolCall) -> ConversationAction {
    let tool_name = tool_call.tool_type.wire_name().to_string();
    let result = run_tool(&conversation_id, tool_call.tool_type).await;
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
