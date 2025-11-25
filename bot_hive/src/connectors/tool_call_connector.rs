use serde::Deserialize;
use std::sync::Arc;

use crate::{
    models::user::{ToolCall, UserAction},
    Env,
};

pub async fn execute_tool(_env: Arc<Env>, tool_call: ToolCall) -> UserAction {
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
}
