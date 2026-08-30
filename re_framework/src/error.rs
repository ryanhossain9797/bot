use thiserror::Error;

#[derive(Error, Debug)]
pub enum ReFrameworkError {
    #[error("State serialization error: {0}")]
    StateSerializationError(#[from] serde_json::Error),
    #[error("Store error: {0}")]
    StoreError(#[from] Box<dyn std::error::Error + Send + Sync>),
}
