// src/io/mod.rs
//
// Sync I/O always available, async I/O with tokio feature

mod common;

// Sync I/O - always available
mod sync;
pub use sync::{FactStreamReader, FactStreamWriter};

// Async I/O - only with tokio feature
#[cfg(feature = "tokio")]
mod asyncio;

#[cfg(feature = "tokio")]
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
