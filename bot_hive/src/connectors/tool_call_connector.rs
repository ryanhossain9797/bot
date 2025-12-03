use serde::Deserialize;
use std::sync::Arc;
use scraper::{Html, Selector};

use crate::{
    configuration::client_tokens::BRAVE_SEARCH_TOKEN,
    models::user::{MathOperation, ToolCall, UserAction},
    Env,
};

pub async fn execute_tool(env: Arc<Env>, tool_call: ToolCall) -> UserAction {
    match tool_call {
        ToolCall::GetWeather { location } => {
            // Actually fetch weather using wttr.in API
            match fetch_weather(&location).await {
                Ok(weather_info) => UserAction::ToolResult(Ok(format!(
                    "Weather for {}: {}",
                    location, weather_info
                ))),
                Err(e) => UserAction::ToolResult(Err(e.to_string())),
            }
        }
        ToolCall::WebSearch { query } => match fetch_web_search(&query).await {
            Ok(search_results) => UserAction::ToolResult(Ok(search_results)),
            Err(e) => UserAction::ToolResult(Err(e.to_string())),
        }
        ToolCall::MathCalculation { operations } => {
            let result = execute_math(operations).await;
            UserAction::ToolResult(Ok(result))
        }
        ToolCall::VisitUrl { url } => {
            match fetch_url_content(&url).await {
                Ok(content) => UserAction::ToolResult(Ok(content)),
                Err(e) => UserAction::ToolResult(Err(e.to_string())),
            }
        }
    }
}

/// Execute a list of math operations and return the results
async fn execute_math(operations: Vec<MathOperation>) -> String {
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
    
    results.join("\n")
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

async fn fetch_weather(location: &str) -> anyhow::Result<String> {
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
    Ok(format!(
        "Temperature: {}°C, Humidity: {}%, Wind Speed: {} km/h",
        weather.temperature_2m, weather.relative_humidity_2m, weather.wind_speed_10m
    ))
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

async fn fetch_web_search(query: &str) -> anyhow::Result<String> {
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
        .take(5)
        .map(|result| {
            let title = result.title.as_deref().unwrap_or("null");
            let url = result.url.as_deref().unwrap_or("null");
            let description = result.description.as_deref().unwrap_or("null");
            // Truncate description to 200 characters max (at character boundary)
            let truncated_description = if description.chars().count() > 200 {
                let truncated: String = description.chars().take(200).collect();
                format!("{}...", truncated)
            } else {
                description.to_string()
            };
            format!(
                "Title: {}\nURL: {}\nDescription: {}",
                title, url, truncated_description
            )
        })
        .collect();

    let formatted_output = format!(
        "Search query: {}\n\nResults:\n{}",
        original_query,
        formatted_results.join("\n\n---\n\n")
    );

    Ok(formatted_output)
}

async fn fetch_url_content(url: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch URL: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "HTTP error {}: {}",
            status,
            status.canonical_reason().unwrap_or("Unknown error")
        ));
    }

    let html_content = response
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?;

    // Parse HTML and extract readable content
    let document = Html::parse_document(&html_content);
    
    // Try to find main content areas in order of preference
    let content_selectors = vec![
        Selector::parse("article").ok(),
        Selector::parse("main").ok(),
        Selector::parse("[role='main']").ok(),
        Selector::parse(".content, .post, .entry, .article-content").ok(),
        Selector::parse("body").ok(),
    ];

    let mut extracted_text = String::new();
    
    for selector_opt in content_selectors {
        if let Some(selector) = selector_opt {
            if let Some(element) = document.select(&selector).next() {
                // Extract text from this element and its children
                let text = element.text().collect::<Vec<_>>().join(" ");
                
                // Clean up whitespace: collapse multiple spaces/newlines into single spaces
                let cleaned: String = text
                    .lines()
                    .map(|line| line.trim())
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                
                if !cleaned.is_empty() && cleaned.len() > 100 {
                    // Found substantial content
                    extracted_text = cleaned;
                    break;
                }
            }
        }
    }

    // Fallback: extract all text from body if no main content found
    if extracted_text.is_empty() {
        let body_selector = Selector::parse("body").unwrap();
        if let Some(body) = document.select(&body_selector).next() {
            extracted_text = body.text().collect::<Vec<_>>().join(" ");
        } else {
            // Last resort: use entire document
            extracted_text = document.root_element().text().collect::<Vec<_>>().join(" ");
        }
    }

    // Clean up: remove excessive whitespace, normalize newlines
    let cleaned_text: String = extracted_text
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    // Limit content length to avoid overwhelming the LLM (keep first 8000 characters)
    let max_chars = 8000;
    let char_count = cleaned_text.chars().count();
    let final_text = if char_count > max_chars {
        // Safely truncate at character boundary (not byte boundary)
        let truncated: String = cleaned_text.chars().take(max_chars).collect();
        format!("{}...\n\n[Content truncated - original length: {} characters]", 
                truncated, char_count)
    } else {
        cleaned_text
    };

    Ok(format!("URL: {}\n\nContent:\n{}", url, final_text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_weather() {
        let weather = fetch_weather("London").await.unwrap();
        assert!(weather.contains("Temperature"));
        assert!(weather.contains("Humidity"));
        assert!(weather.contains("Wind Speed"));
    }

    #[tokio::test]
    async fn test_fetch_web_search() {
        let search_results = fetch_web_search("Rust programming").await.unwrap();
        assert!(search_results.contains("Search query:"));
        assert!(search_results.contains("Results:"));
        assert!(search_results.contains("Rust programming"));
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
        assert!(result.contains("5 + 3 = 8"));
        assert!(result.contains("10 - 4 = 6"));
        assert!(result.contains("6 × 7 = 42"));
        assert!(result.contains("20 ÷ 4 = 5"));
        assert!(result.contains("2 ^ 8 = 256"));
    }

    #[tokio::test]
    async fn test_division_by_zero() {
        let operations = vec![MathOperation::Div(10.0, 0.0)];
        let result = execute_math(operations).await;
        assert!(result.contains("Division by zero"));
    }

    #[tokio::test]
    async fn test_float_operations() {
        let operations = vec![
            MathOperation::Add(5.5, 3.2),
            MathOperation::Div(7.0, 2.0),
            MathOperation::Exp(2.0, 0.5), // Square root via exponentiation
        ];

        let result = execute_math(operations).await;
        assert!(result.contains("5.5 + 3.2 = 8.7"));
        assert!(result.contains("7 ÷ 2 = 3.5"));
        assert!(result.contains("2 ^ 0.5")); // Should calculate sqrt(2)
    }
}
