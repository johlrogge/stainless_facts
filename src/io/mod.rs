// src/io/mod.rs
//
// Sync I/O with io feature, async I/O with tokio feature

mod common;

// Sync I/O - available whenever this module is (io feature)
mod sync;
pub use sync::{FactStreamReader, FactStreamWriter};

// Async I/O - only with tokio feature
#[cfg(feature = "tokio")]
mod asyncio;

#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub use asyncio::{AsyncFactStreamReader, AsyncFactStreamWriter};

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
