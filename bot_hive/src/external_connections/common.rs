use std::{collections::HashMap, fs::read_to_string};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Configuration {
    client_tokens: Option<HashMap<String, String>>,
}

static CONFIGURATION: Lazy<Option<Configuration>> = Lazy::new(|| {
    let configuration: Configuration =
        serde_json::from_str((read_to_string("configuration.json").ok()?).as_str()).ok()?;

    Some(configuration)
});

pub fn get_client_token(client_key: &str) -> Option<&'static str> {
    CONFIGURATION
        .as_ref()?
        .client_tokens
        .as_ref()?
        .get(client_key)
        .map(AsRef::as_ref)
}
