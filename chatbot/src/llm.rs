//! The LLM backend behind `externals::llm_external`. Callers speak the Anthropic Messages API
//! (`anthropic`); `configuration::llm::BACKEND` picks whether that runs on the in-process
//! llama.cpp models (`Native`) or an Anthropic-compatible HTTP endpoint (`Remote`).

pub mod anthropic;
mod native;
mod remote;

use std::sync::Arc;

use anthropic::{MessagesRequest, MessagesResponse};

pub use native::NativeLlm;
pub use remote::RemoteLlm;

/// Which model a request is for. Native maps these to the primary/utility model packs; remote
/// maps them to `RemoteLlmConfig::{primary_model, utility_model}`.
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

#[allow(dead_code)]
#[derive(Clone)]
pub struct RemoteLlmConfig {
    /// Base URL of an Anthropic-compatible API, e.g. `https://api.anthropic.com`;
    /// requests go to `{base_url}/v1/messages`.
    pub base_url: &'static str,
    /// Sent as `x-api-key`.
    pub api_key: &'static str,
    /// Sent as `anthropic-version`.
    pub api_version: &'static str,
    pub primary_model: &'static str,
    /// `None` disables compaction, as a missing utility pack does natively.
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
        self: Arc<Self>,
        role: ModelRole,
        request: MessagesRequest,
    ) -> anyhow::Result<MessagesResponse> {
        match &*self {
            LlmBackend::Native(native) => native.complete(role, request).await,
            LlmBackend::Remote(remote) => remote.complete(role, request).await,
        }
    }
}
