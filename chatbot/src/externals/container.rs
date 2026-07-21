use std::process::Output;
use tokio::process::Command;

pub(crate) async fn docker(args: &[&str]) -> Result<Output, String> {
    Command::new("docker")
        .args(args)
        .output()
        .await
        .map_err(|e| format!("failed to invoke docker: {e}"))
}

pub(crate) async fn is_running(name: &str) -> bool {
    match docker(&["inspect", "-f", "{{.State.Running}}", name]).await {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(_) => false,
    }
}

pub(crate) async fn ensure_image(image: &str, context: &str) -> Result<(), String> {
    let present = docker(&["image", "inspect", image])
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if present {
        return Ok(());
    }
    let out = docker(&["build", "-t", image, context]).await?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "could not build image {image}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

pub(crate) async fn revive_if_present(name: &str) -> bool {
    match docker(&["inspect", "-f", "{{.State.Running}}", name]).await {
        Ok(out) if out.status.success() => {
            if String::from_utf8_lossy(&out.stdout).trim() == "true" {
                return true;
            }
            if docker(&["start", name]).await.map(|o| o.status.success()).unwrap_or(false) {
                return true;
            }
            let _ = docker(&["rm", "-f", name]).await;
            false
        }
        _ => false,
    }
}
