use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum RegistryError {
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    #[error("invalid config: {0}")]
    Config(&'static str),

    #[error("invalid response: {0}")]
    Response(String),

    #[cfg(feature = "cache")]
    #[error("storage error: {0}")]
    Storage(#[from] common::storage::StorageError),

    // TODO: we should make this error serializable instead.
    #[cfg(feature = "cache")]
    #[error("cached error")]
    Cached,
}
