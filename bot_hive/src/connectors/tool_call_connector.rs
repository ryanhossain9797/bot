use std::sync::Arc;

use crate::{
    models::user::{ToolCall, UserAction},
    Env,
};

pub async fn execute_tool(_env: Arc<Env>, tool_call: ToolCall) -> UserAction {
    let result = match tool_call {
        ToolCall::DeviceControl {
            device,
            property,
            value,
        } => {
            // Fake tool execution for now
            format!(
                "Tool call set {} {} {} | Result: Success",
                device, property, value
            )
        }
        ToolCall::GetWeather { location } => {
            // Actually fetch weather using wttr.in API
            match fetch_weather(&location).await {
                Ok(weather_info) => format!("Weather for {}: {}", location, weather_info),
                Err(e) => format!("Failed to get weather for {}: {}", location, e),
            }
        }
    };

    UserAction::ToolResult(Arc::new(Ok(result)))
}

async fn fetch_weather(location: &str) -> anyhow::Result<String> {
    // Use wttr.in API - free, no API key required
    // Format: ?format=... gives us just the data we need
    let url = format!(
        "https://wttr.in/{}?format=%C+%t+%w+%h",
        urlencoding::encode(location)
    );

    let response = reqwest::get(&url).await?;
    let status = response.status();

    if !status.is_success() {
        return Err(anyhow::anyhow!("Weather API returned status: {}", status));
    }

    let text = response.text().await?;
    let trimmed = text.trim();

    if trimmed.is_empty() {
        return Err(anyhow::anyhow!("Empty response from weather API"));
    }

    // Parse the format: Condition Temperature Wind Humidity
    // Example: "Clear +15°C 10km/h 65%"
    Ok(trimmed.to_string())
}
