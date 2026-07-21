use crate::externals::container::{docker, ensure_image, is_running, revive_if_present};
use crate::types::conversation::ToolResultData;
use crate::types::media::{Image, MessageImage};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const WORKER_IMAGE: &str = "bot-worker:latest";
const WORKER_BUILD_CONTEXT: &str = "/app/worker";

fn worker_name(conversation_id: &str) -> String {
    let safe: String = conversation_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    format!("botwork-{safe}")
}

async fn ensure_worker(name: &str) -> Result<(), String> {
    if revive_if_present(name).await {
        return Ok(());
    }
    spawn_worker(name).await
}

pub(crate) async fn ensure_worker_image() -> Result<(), String> {
    ensure_image(WORKER_IMAGE, WORKER_BUILD_CONTEXT).await
}

async fn spawn_worker(name: &str) -> Result<(), String> {
    ensure_worker_image().await?;
    let out = docker(&[
        "run", "-d", "--name", name,
        "--memory", "1g", "--cpus", "2", "--pids-limit", "512",
        "--security-opt", "no-new-privileges",
        WORKER_IMAGE, "sleep", "infinity",
    ])
    .await?;
    if out.status.success() {
        return Ok(());
    }
    if is_running(name).await {
        return Ok(());
    }
    Err(format!(
        "could not start sandbox: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

pub(crate) const ACTUAL_MAX: usize = 20_000;
pub(crate) const SIMPLIFIED_MAX: usize = 2_000;

pub(crate) fn clip_to(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let head: String = s.chars().take(max).collect();
        format!("{head}\n…[output truncated]")
    } else {
        s.to_string()
    }
}

pub async fn run_bash(conversation_id: &str, command: &str) -> Result<ToolResultData, String> {
    let name = worker_name(conversation_id);
    ensure_worker(&name).await?;

    let out = docker(&["exec", &name, "bash", "-c", command]).await?;
    let stdout = clip_to(&String::from_utf8_lossy(&out.stdout), ACTUAL_MAX);
    let stderr = clip_to(&String::from_utf8_lossy(&out.stderr), ACTUAL_MAX);
    let code = out.status.code().unwrap_or(-1);

    let body = format!("exit code: {code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    let simplified = clip_to(&body, SIMPLIFIED_MAX);
    Ok(ToolResultData::text(body, simplified))
}

pub async fn pull_image(conversation_id: &str, path: &str) -> Result<MessageImage, String> {
    let name = worker_name(conversation_id);
    ensure_worker(&name).await?;

    let out = docker(&["exec", &name, "cat", "--", path]).await?;
    if !out.status.success() {
        return Err(format!(
            "could not read '{path}': {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let bytes = out.stdout;
    let format = image::guess_format(&bytes)
        .map_err(|_| format!("'{path}' is not a valid image (expected PNG, JPEG, GIF, or WebP)"))?;
    let image = Image {
        bytes: Arc::new(bytes),
        mime: format.to_mime_type().to_string(),
    };
    Ok(MessageImage::Hydrated(image).downscaled())
}

pub struct PulledFile {
    pub filename: String,
    pub bytes: Vec<u8>,
}

pub(crate) const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;

pub async fn pull_file(conversation_id: &str, path: &str) -> Result<PulledFile, String> {
    let name = worker_name(conversation_id);
    ensure_worker(&name).await?;

    let out = docker(&["exec", &name, "cat", "--", path]).await?;
    if !out.status.success() {
        return Err(format!(
            "could not read '{path}': {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let bytes = out.stdout;
    if bytes.is_empty() {
        return Err(format!("'{path}' is empty — nothing to attach"));
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "'{path}' is {} bytes, over the {MAX_ATTACHMENT_BYTES}-byte attachment limit",
            bytes.len()
        ));
    }

    let filename = path
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("attachment")
        .to_string();
    Ok(PulledFile { filename, bytes })
}

pub async fn read_file(conversation_id: &str, path: &str) -> Result<String, String> {
    let name = worker_name(conversation_id);
    ensure_worker(&name).await?;

    let out = docker(&["exec", &name, "cat", "--", path]).await?;
    if !out.status.success() {
        return Err(format!(
            "could not read '{path}': {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub async fn write_file(conversation_id: &str, path: &str, content: &str) -> Result<(), String> {
    let name = worker_name(conversation_id);
    ensure_worker(&name).await?;

    let mut child = Command::new("docker")
        .args(["exec", "-i", &name, "tee", "--", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to invoke docker: {e}"))?;

    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(content.as_bytes())
        .await
        .map_err(|e| format!("failed to stream '{path}' to sandbox: {e}"))?;

    let out = child
        .wait_with_output()
        .await
        .map_err(|e| format!("failed to write '{path}': {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "could not write '{path}': {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

pub async fn reset_bash(conversation_id: &str) -> Result<ToolResultData, String> {
    let name = worker_name(conversation_id);
    let _ = docker(&["rm", "-f", &name]).await;
    let msg = "Sandbox reset — a fresh environment starts on the next command.".to_string();
    Ok(ToolResultData::text(msg.clone(), msg))
}
