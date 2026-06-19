use crate::types::conversation::ToolResultData;
use crate::types::media::{Image, MessageImage};
use std::sync::Arc;
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

async fn docker(args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("docker")
        .args(args)
        .output()
        .await
        .map_err(|e| format!("failed to invoke docker: {e}"))
}

async fn is_running(name: &str) -> bool {
    match docker(&["inspect", "-f", "{{.State.Running}}", name]).await {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(_) => false,
    }
}

async fn ensure_worker(name: &str) -> Result<(), String> {
    match docker(&["inspect", "-f", "{{.State.Running}}", name]).await {
        Ok(out) if out.status.success() => {
            if String::from_utf8_lossy(&out.stdout).trim() == "true" {
                return Ok(()); 
            }
            if docker(&["start", name]).await.map(|o| o.status.success()).unwrap_or(false) {
                return Ok(());
            }
            let _ = docker(&["rm", "-f", name]).await; 
        }
        _ => {} 
    }
    spawn_worker(name).await
}

pub(crate) async fn ensure_worker_image() -> Result<(), String> {
    let present = docker(&["image", "inspect", WORKER_IMAGE])
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if present {
        return Ok(());
    }
    let out = docker(&["build", "-t", WORKER_IMAGE, WORKER_BUILD_CONTEXT]).await?;
    if !out.status.success() {
        return Err(format!(
            "could not build sandbox image: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
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

fn clip(s: &str) -> String {
    const MAX: usize = 20_000;
    if s.chars().count() > MAX {
        let head: String = s.chars().take(MAX).collect();
        format!("{head}\n…[output truncated]")
    } else {
        s.to_string()
    }
}

pub async fn run_bash(conversation_id: &str, command: &str) -> Result<ToolResultData, String> {
    let name = worker_name(conversation_id);
    ensure_worker(&name).await?;

    let out = docker(&["exec", &name, "bash", "-c", command]).await?;
    let stdout = clip(&String::from_utf8_lossy(&out.stdout));
    let stderr = clip(&String::from_utf8_lossy(&out.stderr));
    let code = out.status.code().unwrap_or(-1);

    let body = format!("exit code: {code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    Ok(ToolResultData::text(body.clone(), body))
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

pub async fn reset_bash(conversation_id: &str) -> Result<ToolResultData, String> {
    let name = worker_name(conversation_id);
    let _ = docker(&["rm", "-f", &name]).await;
    let msg = "Sandbox reset — a fresh environment starts on the next command.".to_string();
    Ok(ToolResultData::text(msg.clone(), msg))
}
