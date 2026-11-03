pub mod anthropic;
mod images;
mod native;
mod remote;

use anthropic::{MessagesRequest, MessagesResponse, StopReason};

pub use native::NativeLlm;
#[cfg(test)]
pub(crate) use native::render_with_pack;
pub use remote::RemoteLlm;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRole {
    Primary,
    Utility,
}

#[allow(dead_code)]
pub enum LlmBackendConfig {
    Native,
    Remote(RemoteLlmConfig),
}

#[derive(Clone)]
pub struct RemoteLlmConfig {
    pub base_url: &'static str,
    pub api_key: &'static str,
    pub api_version: &'static str,
    pub primary_model: &'static str,
    pub utility_model: Option<&'static str>,
    pub timeout_secs: u64,
}

pub enum LlmBackend {
    Native(NativeLlm),
    Remote(RemoteLlm),
}

impl LlmBackend {
    pub fn has(&self, role: ModelRole) -> bool {
        match self {
            LlmBackend::Native(native) => native.has(role),
            LlmBackend::Remote(remote) => remote.has(role),
        }
    }

    pub async fn complete(
        &self,
        role: ModelRole,
        mut request: MessagesRequest,
    ) -> anyhow::Result<MessagesResponse> {
        let mut messages = std::mem::take(&mut request.messages);
        request.messages = tokio::task::spawn_blocking(move || {
            images::sanitize(&mut messages);
            messages
        })
        .await?;
        let max_tokens = request.max_tokens;
        let response = match self {
            LlmBackend::Native(native) => native.complete(role, request).await,
            LlmBackend::Remote(remote) => remote.complete(role, request).await,
        }?;
        if response.stop_reason == Some(StopReason::MaxTokens) {
            eprintln!(
                "[llm] {role:?} reply hit max_tokens ({max_tokens}); it may be cut off, or hold only thinking"
            );
        }
        Ok(response)
    }
}
