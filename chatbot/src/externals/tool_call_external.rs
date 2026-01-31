use crate::{
    configuration::client_tokens::BRAVE_SEARCH_TOKEN,
    models::user::{
        HistoryEntry, MathOperation, ToolCall, ToolResultData, UserAction,
        MAX_SEARCH_DESCRIPTION_LENGTH,
    },
    Env,
};
use scraper::{Html, Selector};
use serde::Deserialize;
use std::collections::HashSet;
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

    let actual = format!("MATH TOOL RESULT:\n{}", results.join("\n"));

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
        "WEATHER TOOL RESULT: Temperature: {}°C, Humidity: {}%, Wind Speed: {} km/h",
        weather.temperature_2m, weather.relative_humidity_2m, weather.wind_speed_10m
    );
    Ok(ToolResultData {
        simplified: actual.clone(),
        actual,
    })
}

#[derive(Deserialize)]
struct BraveSearchResponse {
    query: BraveSearchQuery,
    web: BraveWebResults,
}

#[derive(Deserialize)]
struct BraveSearchQuery {
    original: String,
}

#[derive(Deserialize)]
struct BraveWebResults {
    results: Vec<BraveSearchResult>,
}

#[derive(Deserialize)]
struct BraveSearchResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

async fn fetch_web_search(query: &str) -> anyhow::Result<ToolResultData> {
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
        .take(3)
        .map(|result| {
            let title = result.title.as_deref().unwrap_or("null");
            let url = result.url.as_deref().unwrap_or("null");
            let description = result.description.as_deref().unwrap_or("null");

            let safe_len = description.floor_char_boundary(MAX_SEARCH_DESCRIPTION_LENGTH);
            let description = &description[..safe_len];
            format!("Title: {title}\nURL to visit: {url}\nDescription: {description}\n\n",)
        })
        .collect();

    let simplified_partition = 1;
    let (primary, secondary) = match formatted_results.len() > simplified_partition {
        true => formatted_results.split_at(simplified_partition),
        false => (formatted_results.as_slice(), &[][..]),
    };

    let simplified = format!(
        "WEB SEARCH TOOL RESULT: Search Results for {}:\n{}",
        original_query,
        primary.join("\n")
    );

    let actual = format!("{simplified}\n{}", secondary.join("\n"));

    Ok(ToolResultData { actual, simplified })
}

#[derive(Debug)]
struct ExtractedPage {
    final_url: String,
    content: String,
    links: Vec<(String, String)>,
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

    // println!("DEBUG: Raw HTML fetched: {}", html_body);

    // Readability extraction
    let mut cursor = std::io::Cursor::new(html_body.as_bytes());
    let url_obj = reqwest::Url::parse(&final_url)
        .map_err(|e| anyhow::anyhow!("Failed to parse final URL: {}", e))?;

    let product = readability::extractor::extract(&mut cursor, &url_obj)
        .map_err(|e| anyhow::anyhow!("Readability extraction failed: {}", e))?;

    let content_html = product.content;
    let page_title = product.title;

    // Scraper for text and link extraction
    let fragment = Html::parse_fragment(&content_html);

    // Extract text
    let mut text_parts = Vec::new();

    // Add title first if present
    if !page_title.is_empty() {
        text_parts.push(page_title);
    }

    // Select block elements to preserve some structure
    let block_selector = Selector::parse("p, h1, h2, h3, h4, h5, h6, li, div").unwrap();

    for element in fragment.select(&block_selector) {
        let text = element.text().collect::<Vec<_>>().join(" ");
        let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !cleaned.is_empty() {
            text_parts.push(cleaned);
        }
    }

    // Fallback if no blocks found (unlikely with readability)
    if text_parts.is_empty() {
        let text = fragment.root_element().text().collect::<Vec<_>>().join(" ");
        let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !cleaned.is_empty() {
            text_parts.push(cleaned);
        }
    }

    let clean_text = text_parts.join("\n\n");

    // Link extraction
    let link_selector = Selector::parse("a").unwrap();
    let mut links = Vec::new();
    let mut seen_links = HashSet::new();

    for element in fragment.select(&link_selector) {
        if let Some(href) = element.value().attr("href") {
            let text = element.text().collect::<Vec<_>>().join(" ");
            let text_trimmed = text.trim();

            if !text_trimmed.is_empty() && !href.is_empty() {
                // Deduplicate by href
                if seen_links.insert(href.to_string()) {
                    links.push((text_trimmed.to_string(), href.to_string()));
                }
            }
        }
    }

    Ok(ExtractedPage {
        final_url,
        content: clean_text,
        links,
    })
}

async fn fetch_url_content(url: &str) -> anyhow::Result<ToolResultData> {
    pub const MAX_ACTUAL_WEB_CONTENT_LENGTH: usize = 10000;
    pub const MAX_SIMPLIFIED_WEB_CONTENT_LENGTH: usize = 300;

    let extracted = fetch_page(url).await?;

    let actual_content = if extracted.content.len() > MAX_ACTUAL_WEB_CONTENT_LENGTH {
        &extracted.content[..MAX_ACTUAL_WEB_CONTENT_LENGTH]
    } else {
        &extracted.content
    };

    let simplified_content = if extracted.content.len() > MAX_SIMPLIFIED_WEB_CONTENT_LENGTH {
        &extracted.content[..MAX_SIMPLIFIED_WEB_CONTENT_LENGTH]
    } else {
        &extracted.content
    };

    let mut actual: String = format!("VISIT URL TOOL RESULT {url}: \n");
    let mut simplified = actual.clone();
    actual.push_str(actual_content);
    simplified.push_str(simplified_content);

    if !extracted.links.is_empty() {
        actual.push_str("\nLinks:\n");
        simplified.push_str("\nLinks:\n");
        for (index, (text, href)) in extracted.links.iter().enumerate().take(10) {
            actual.push_str(&format!("- {} {}\n", text, href));
            if index < 3 {
                simplified.push_str(&format!("- {} {}\n", text, href));
            }
        }
    }

    Ok(ToolResultData { actual, simplified })
}

#[allow(unused_variables)]
pub async fn execute_tool(
    env: Arc<Env>,
    tool_call: ToolCall,
    history: Vec<HistoryEntry>,
) -> UserAction {
    match tool_call {
        ToolCall::GetWeather { location } => match fetch_weather(&location).await {
            Ok(weather_info) => UserAction::ToolResult(Ok(weather_info)),
            Err(e) => UserAction::ToolResult(Err(e.to_string())),
        },
        ToolCall::WebSearch { query } => match fetch_web_search(&query).await {
            Ok(search_results) => UserAction::ToolResult(Ok(search_results)),
            Err(e) => UserAction::ToolResult(Err(e.to_string())),
        },
        ToolCall::MathCalculation { operations } => {
            let result = execute_math(operations).await;
            UserAction::ToolResult(Ok(result))
        }
        ToolCall::VisitUrl { url } => match fetch_url_content(&url).await {
            Ok(content) => UserAction::ToolResult(Ok(content)),
            Err(e) => UserAction::ToolResult(Err(e.to_string())),
        },
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

    #[tokio::test]
    async fn test_fetch_web_search() {
        let search_results = fetch_web_search("Rust programming").await.unwrap();
        assert!(search_results.actual.contains("Search Results for"));
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
