use crate::{
    configuration::client_tokens::{BRAVE_SEARCH_TOKEN, SEARXNG_URL},
    externals::{recall_long_term_external::recall_long, recall_short_term_external::recall_short},
    types::conversation::{
        HistoryEntry, MathOperation, ToolCall, ToolResultData, ToolType, ConversationAction,
        MAX_SEARCH_DESCRIPTION_LENGTH,
    },
    Env,
};
use rs_trafilatura::extract;
use serde::Deserialize;
use std::sync::Arc;

/// Execute a list of math operations and return the results
async fn execute_math(operations: Vec<MathOperation>) -> ToolResultData {
    let mut results = Vec::new();

    for (index, op) in operations.iter().enumerate() {
        let result = match op {
            MathOperation::Add(a, b) => {
                let res = *a + *b;
                format!("{} + {} = {}", a, b, res)
            }
            MathOperation::Sub(a, b) => {
                let res = *a - *b;
                format!("{} - {} = {}", a, b, res)
            }
            MathOperation::Mul(a, b) => {
                let res = *a * *b;
                format!("{} × {} = {}", a, b, res)
            }
            MathOperation::Div(a, b) => {
                if *b == 0.0 {
                    format!("{} ÷ {} = Error: Division by zero", a, b)
                } else {
                    let res = *a / *b;
                    format!("{} ÷ {} = {}", a, b, res)
                }
            }
            MathOperation::Exp(a, b) => {
                let res = (*a as f64).powf(*b as f64);
                format!("{} ^ {} = {}", a, b, res)
            }
        };
        results.push(format!("Operation {}: {}", index + 1, result));
    }

    let actual = format!("Calculation results:\n{}", results.join("\n"));

    ToolResultData {
        simplified: actual.clone(),
        actual,
    }
}

#[derive(Deserialize)]
struct GeocodingResponse {
    results: Option<Vec<GeocodingResult>>,
}

#[derive(Deserialize)]
struct GeocodingResult {
    latitude: f64,
    longitude: f64,
}

#[derive(Deserialize)]
struct WeatherResponse {
    current: CurrentWeather,
}

#[derive(Deserialize)]
struct CurrentWeather {
    temperature_2m: f64,
    relative_humidity_2m: u32,
    wind_speed_10m: f64,
}

async fn fetch_weather(location: &str) -> anyhow::Result<ToolResultData> {
    let geocoding_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1",
        urlencoding::encode(location)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let geocoding_response = client
        .get(&geocoding_url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to geocoding service: {}", e))?
        .json::<GeocodingResponse>()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse geocoding response: {}", e))?;

    let result = geocoding_response
        .results
        .and_then(|mut r| r.pop())
        .ok_or_else(|| anyhow::anyhow!("Location '{}' not found.", location))?;

    let weather_url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,relative_humidity_2m,wind_speed_10m",
        result.latitude, result.longitude
    );

    let weather_response = client
        .get(&weather_url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to weather service: {}", e))?
        .json::<WeatherResponse>()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse weather response: {}", e))?;

    let weather = weather_response.current;

    let actual = format!(
        "Temperature: {}°C, Humidity: {}%, Wind Speed: {} km/h",
        weather.temperature_2m, weather.relative_humidity_2m, weather.wind_speed_10m
    );
    Ok(ToolResultData {
        simplified: actual.clone(),
        actual,
    })
}

/// SearxNG `/search?format=json` response. Beyond the ranked `results`, SearxNG aggregates an
/// instant `answers` summary and structured `infoboxes` (entity cards) — these are often a direct
/// answer to the query, so we surface them ahead of the links. Other top-level fields
/// (suggestions, corrections, …) are ignored.
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
    /// The infobox heading (entity name), e.g. "Rust (programming language)".
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
    /// SearxNG's result snippet (mapped to our "Description" line).
    #[serde(default)]
    content: Option<String>,
    /// Publication date, when the engine reports one (news/articles); absent for most pages.
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
}

/// Search results formatted for the model. The 32k-token context affords plenty of room; the old
/// cap of 3 was tuned for a much smaller, less capable setup.
const MAX_SEARCH_RESULTS: usize = 8;
/// Of those, how many also go into the `simplified` (recall/memory) text.
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

    // High-signal lead: instant answers and the first infobox, when present — often a direct
    // answer to the query, placed ahead of the ranked links. Shared by both simplified and actual.
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
            // Surface the publication date when the engine provides one (recency cue).
            let published = result
                .published_date
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|d| format!("Published: {d}\n"))
                .unwrap_or_default();
            format!("Title: {title}\nURL to visit: {url}\n{published}Description: {description}\n\n")
        })
        .collect();

    // `simplified` (recall/memory) keeps the lead plus the top few results; `actual` (fed to the
    // model live) keeps the lead plus every result.
    let split = SIMPLIFIED_SEARCH_RESULTS.min(formatted_results.len());
    let (primary, secondary) = formatted_results.split_at(split);

    let simplified = format!(
        "{lead}Search results for \"{original_query}\":\n{}",
        primary.join("\n")
    );

    let actual = format!("{simplified}\n{}", secondary.join("\n"));

    Ok(ToolResultData { actual, simplified })
}

// ---------------------------------------------------------------------------------------------
// Dormant fallback: the legacy Brave Search path, kept intact but NOT wired into `web_search`
// (the active tool uses `fetch_web_search` / SearxNG above). Left in place so we can fall back by
// swapping the call in `run_tool`. Remove once SearxNG is proven out. See #113 / closed #112.
// ---------------------------------------------------------------------------------------------
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
    // Fetch HTML content from URL
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

    // Check content type
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

    // // Convert HTML to Markdown using the service
    // let markdown = markdown_service.convert(&html_body);

    let content = extract(&html_body)?.content_text;

    Ok(ExtractedPage { final_url, content })
}

async fn fetch_url_content(url: &str) -> anyhow::Result<ToolResultData> {
    // Generous caps for the 32k-token context: `actual` is the full read fed to the model,
    // `simplified` a longer preview for recall/memory (were 10000 / 300).
    pub const MAX_ACTUAL_WEB_CONTENT_LENGTH: usize = 50000;
    pub const MAX_SIMPLIFIED_WEB_CONTENT_LENGTH: usize = 2000;

    let extracted = fetch_page(url).await?;

    // `floor_char_boundary` clamps to the string length and never splits a multi-byte char, so a
    // raw byte-index slice here can't panic on a UTF-8 boundary (the old `[..N]` could).
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

/// Run one tool to its result. Errors are returned as `Err(String)`; the state machine folds them
/// into a `ToolResultData` when moving the call to `completed_tools`.
async fn run_tool(
    env: Arc<Env>,
    tool_type: ToolType,
    conversation_id: String,
    history: Vec<HistoryEntry>,
) -> Result<ToolResultData, String> {
    match tool_type {
        ToolType::GetWeather { location } => fetch_weather(&location).await.map_err(|e| e.to_string()),
        ToolType::MathCalculation { operations } => Ok(execute_math(operations).await),
        ToolType::WebSearch { query } => fetch_web_search(&query).await.map_err(|e| e.to_string()),
        ToolType::VisitUrl { url } => fetch_url_content(&url).await.map_err(|e| e.to_string()),
        ToolType::RecallShortTerm { .. } => Ok(recall_short(&history)),
        ToolType::RecallLongTerm { search_term } => {
            recall_long(env, conversation_id, search_term).await.map_err(|e| e.to_string())
        }
    }
}

/// Execute a single tool call as its own external op, tagging the result action with the call's id
/// so the state machine can move it from `pending_tools` to `completed_tools`.
pub async fn execute_tool(
    env: Arc<Env>,
    tool_call: ToolCall,
    conversation_id: String,
    history: Vec<HistoryEntry>,
) -> ConversationAction {
    let result = run_tool(env, tool_call.tool_type, conversation_id, history).await;
    ConversationAction::ToolResult {
        id: tool_call.id,
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_weather() {
        let weather = fetch_weather("London").await.unwrap();
        assert!(weather.actual.contains("Temperature"));
        assert!(weather.actual.contains("Humidity"));
        assert!(weather.actual.contains("Wind Speed"));
    }

    // Integration test: hits the SearxNG instance at SEARXNG_URL (reachable as localhost:8080 from
    // the host). Requires a running instance with JSON format enabled.
    #[tokio::test]
    async fn test_fetch_web_search() {
        let search_results = fetch_web_search("Rust programming").await.unwrap();
        assert!(search_results.actual.contains("Search results for"));
        assert!(search_results.actual.contains("Rust programming"));
    }

    #[tokio::test]
    async fn test_math_operations() {
        let operations = vec![
            MathOperation::Add(5.0, 3.0),
            MathOperation::Sub(10.0, 4.0),
            MathOperation::Mul(6.0, 7.0),
            MathOperation::Div(20.0, 4.0),
            MathOperation::Exp(2.0, 8.0),
        ];

        let result = execute_math(operations).await;
        assert!(result.actual.contains("5 + 3 = 8"));
        assert!(result.actual.contains("10 - 4 = 6"));
        assert!(result.actual.contains("6 × 7 = 42"));
        assert!(result.actual.contains("20 ÷ 4 = 5"));
        assert!(result.actual.contains("2 ^ 8 = 256"));
    }

    #[tokio::test]
    async fn test_division_by_zero() {
        let operations = vec![MathOperation::Div(10.0, 0.0)];
        let result = execute_math(operations).await;
        assert!(result.actual.contains("Division by zero"));
    }

    #[tokio::test]
    async fn test_float_operations() {
        let operations = vec![
            MathOperation::Add(5.5, 3.2),
            MathOperation::Div(7.0, 2.0),
            MathOperation::Exp(2.0, 0.5), // Square root via exponentiation
        ];

        let result = execute_math(operations).await;
        assert!(result.actual.contains("5.5 + 3.2 = 8.7"));
        assert!(result.actual.contains("7 ÷ 2 = 3.5"));
        assert!(result.actual.contains("2 ^ 0.5")); // Should calculate sqrt(2)
    }

    #[tokio::test]
    async fn test_fetch_url_content_real() {
        // Test with example.com
        let content = fetch_url_content("https://example.com").await.unwrap();
        // println!("{}", content); // Keep it clean
        assert!(content.actual.contains("Example Domain"));
        // The text on example.com seems to vary or has changed.
        // We match parts of the text found in the debug run:
        // "This domain is for use in documentation examples without needing permission."
        assert!(content
            .actual
            .contains("This domain is for use in documentation examples"));
        assert!(content.actual.contains("https://iana.org/domains/example"));
    }
}
