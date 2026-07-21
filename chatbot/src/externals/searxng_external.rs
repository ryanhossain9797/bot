use crate::configuration::client_tokens::SEARXNG_URL;
use crate::externals::container::{docker, ensure_image, is_running, revive_if_present};

const SEARXNG_IMAGE: &str = "bot-searxng:latest";
const SEARXNG_BUILD_CONTEXT: &str = "/app/searxng";
const SEARXNG_CONTAINER: &str = "bot-searxng";
const SEARXNG_PORT_MAP: &str = "8080:8080";

fn is_self_hosted() -> bool {
    let url = SEARXNG_URL.trim();
    url.contains("localhost") || url.contains("127.0.0.1")
}

pub(crate) async fn ensure_searxng_image() -> Result<(), String> {
    ensure_image(SEARXNG_IMAGE, SEARXNG_BUILD_CONTEXT).await
}

pub(crate) async fn ensure_searxng() -> Result<(), String> {
    if !is_self_hosted() {
        return Ok(());
    }
    if revive_if_present(SEARXNG_CONTAINER).await {
        return Ok(());
    }
    spawn_searxng().await
}

async fn spawn_searxng() -> Result<(), String> {
    ensure_searxng_image().await?;
    let out = docker(&[
        "run", "-d", "--name", SEARXNG_CONTAINER,
        "--restart", "unless-stopped",
        "-p", SEARXNG_PORT_MAP,
        "--memory", "512m", "--cpus", "1",
        SEARXNG_IMAGE,
    ])
    .await?;
    if out.status.success() {
        return Ok(());
    }
    if is_running(SEARXNG_CONTAINER).await {
        return Ok(());
    }
    Err(format!(
        "could not start searxng: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}
