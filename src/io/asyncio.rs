use super::{common, ReadError, WriteError};
use crate::Fact;
use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

pub struct AsyncFactStreamWriter {
    sync_file: std::fs::File, // For locking
    writer: BufWriter<File>,
}

impl AsyncFactStreamWriter {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        Self::open_with_timeout(path, Duration::from_secs(0)).await
    }

    pub async fn open_with_timeout(
        path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, WriteError> {
        let path = path.as_ref();

        // Open sync file for locking
        let sync_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        let start = Instant::now();
        let retry_interval = Duration::from_millis(100);

        loop {
            match sync_file.try_lock_exclusive() {
                Ok(()) => {
                    // Convert to async file
                    let std_file = sync_file.try_clone()?;
                    let async_file = File::from_std(std_file);
                    let writer = BufWriter::new(async_file);

                    return Ok(Self { sync_file, writer });
                }
                Err(_) if timeout.is_zero() => {
                    return Err(WriteError::AlreadyLocked);
                }
                Err(_) if start.elapsed() >= timeout => {
                    return Err(WriteError::LockTimeout(timeout));
                }
                Err(_) => {
                    tokio::time::sleep(retry_interval).await;
                }
            }
        }
    }

    pub async fn write_batch<E, V, S>(&mut self, facts: &[Fact<E, V, S>]) -> Result<(), WriteError>
    where
        E: Serialize,
        V: Serialize,
        S: Serialize,
    {
        let buffer = common::serialize_batch(facts)?;
        self.writer.write_all(&buffer).await?;
        self.writer.flush().await?;
        self.writer.get_ref().sync_all().await?;
        Ok(())
    }
}

impl Drop for AsyncFactStreamWriter {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.sync_file);
    }
}

pub struct AsyncFactStreamReader<E, V, S> {
    reader: BufReader<File>,
    _phantom: std::marker::PhantomData<(E, V, S)>,
}

impl<E, V, S> AsyncFactStreamReader<E, V, S> {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, ReadError> {
        Self::open_with_timeout(path, Duration::from_secs(0)).await
    }

    pub async fn open_with_timeout(
        path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, ReadError> {
        let path = path.as_ref();

        let sync_file = std::fs::OpenOptions::new().read(true).open(path)?;

        let start = Instant::now();
        let retry_interval = Duration::from_millis(100);

        loop {
            match FileExt::try_lock_shared(&sync_file) {
                Ok(()) => {
                    let async_file = File::from_std(sync_file);
                    let reader = BufReader::new(async_file);

                    return Ok(Self {
                        reader,
                        _phantom: std::marker::PhantomData,
                    });
                }
                Err(_) if timeout.is_zero() => {
                    return Err(ReadError::AlreadyLocked);
                }
                Err(_) if start.elapsed() >= timeout => {
                    return Err(ReadError::LockTimeout(timeout));
                }
                Err(_) => {
                    tokio::time::sleep(retry_interval).await;
                }
            }
        }
    }

    pub async fn next(&mut self) -> Option<Result<Fact<E, V, S>, ReadError>>
    where
        E: DeserializeOwned,
        V: DeserializeOwned,
        S: DeserializeOwned,
    {
        let mut line = String::new();

        loop {
            line.clear();
            match self.reader.read_line(&mut line).await {
                Ok(0) => return None,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    match serde_json::from_str(trimmed) {
                        Ok(fact) => return Some(Ok(fact)),
                        Err(e) => return Some(Err(ReadError::Deserialization(e))),
                    }
                }
                Err(e) => return Some(Err(ReadError::Io(e))),
            }
        }
    }
}
