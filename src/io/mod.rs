mod common;

#[cfg(not(feature = "tokio"))]
mod sync;
#[cfg(not(feature = "tokio"))]
pub use sync::{FactStreamReader, FactStreamWriter};

#[cfg(feature = "tokio")]
mod asyncio;
#[cfg(feature = "tokio")]
pub use asyncio::{
    AsyncFactStreamReader as FactStreamReader, AsyncFactStreamWriter as FactStreamWriter,
};

use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("File is already locked by another process")]
    AlreadyLocked,

    #[error("Failed to acquire lock within {0:?}")]
    LockTimeout(Duration),
}

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Deserialization failed: {0}")]
    Deserialization(#[from] serde_json::Error),

    #[error("File is already locked by another process")]
    AlreadyLocked,

    #[error("Failed to acquire lock within {0:?}")]
    LockTimeout(Duration),
}
