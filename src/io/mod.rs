// src/io/mod.rs
//
// Sync I/O with io feature, async I/O with tokio feature

mod common;

// Sync I/O - available whenever this module is (io feature)
mod sync;
pub use sync::{read_lines_at, FactStreamReader, FactStreamWriter};

// Async I/O - only with tokio feature
#[cfg(feature = "tokio")]
mod asyncio;

#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub use asyncio::{
    read_lines_at as read_lines_at_async, AsyncFactStreamReader, AsyncFactStreamWriter,
};

#[cfg(test)]
mod tests;

use crate::StreamPosition;
use std::time::Duration;
use thiserror::Error;

/// An error from writing to a fact stream with a [`FactStreamWriter`] (or
/// the async writer, with the `tokio` feature).
///
/// [`Serialization`](Self::Serialization),
/// [`AlreadyLocked`](Self::AlreadyLocked) and
/// [`LockTimeout`](Self::LockTimeout) are reported before anything is
/// written. After an [`Io`](Self::Io) error from a write, the batch may
/// have been written in part or in full.
///
/// New variants may be added in minor releases, so a `match` needs a
/// wildcard arm.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WriteError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Another writer holds the stream's exclusive lock and the writer was
    /// opened with a zero lock timeout.
    #[error("File is already locked by another process")]
    AlreadyLocked,

    /// Another writer held the stream's exclusive lock for longer than the
    /// writer's lock timeout.
    #[error("Failed to acquire lock within {0:?}")]
    LockTimeout(Duration),
}

/// An error from reading a fact stream with a [`FactStreamReader`],
/// [`read_lines_at`] or a store iterator (or their async twins, with the
/// `tokio` feature).
///
/// A [`Deserialization`](Self::Deserialization) error is per line: the
/// readers skip the line and can continue past it. An [`Io`](Self::Io)
/// error is not. [`InvalidPosition`](Self::InvalidPosition) is reported
/// when opening at a position that cannot belong to the stream.
///
/// New variants may be added in minor releases, so a `match` needs a
/// wildcard arm.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Deserialization failed: {0}")]
    Deserialization(#[from] serde_json::Error),

    /// The position is past the end of the stream or not at the start of a
    /// line, so it cannot have come from this stream.
    #[error("Invalid stream position {0:?}: past the end or not at a line boundary")]
    InvalidPosition(StreamPosition),
}
