// stainless_facts/src/io/sync.rs

use super::{common, ReadError, WriteError};
use crate::Fact;
use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

pub struct FactStreamWriter {
    file: File,
    writer: BufWriter<File>,
    lock_timeout: Duration,
}

impl FactStreamWriter {
    /// Open a fact stream file for writing.
    ///
    /// The lock is acquired only during write operations, not continuously.
    /// This allows readers to access the file between writes.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        Self::open_with_timeout(path, Duration::from_secs(0))
    }

    /// Open with a timeout for acquiring the lock during writes.
    pub fn open_with_timeout(
        path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, WriteError> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let writer = BufWriter::new(file.try_clone()?);
        Ok(Self {
            file,
            writer,
            lock_timeout: timeout,
        })
    }

    /// Acquire exclusive lock with configured timeout
    fn acquire_lock(&self) -> Result<(), WriteError> {
        let start = Instant::now();
        let retry_interval = Duration::from_millis(100);

        loop {
            match self.file.try_lock_exclusive() {
                Ok(()) => return Ok(()),
                Err(_) if self.lock_timeout.is_zero() => {
                    return Err(WriteError::AlreadyLocked);
                }
                Err(_) if start.elapsed() >= self.lock_timeout => {
                    return Err(WriteError::LockTimeout(self.lock_timeout));
                }
                Err(_) => {
                    std::thread::sleep(retry_interval);
                }
            }
        }
    }

    /// Write a batch of facts atomically.
    ///
    /// Acquires exclusive lock, writes all facts, then releases lock.
    /// All facts are serialized to memory first. If serialization fails,
    /// no facts are written. After successful write, fsync ensures durability.
    pub fn write_batch<E, V, S>(&mut self, facts: &[Fact<E, V, S>]) -> Result<(), WriteError>
    where
        E: Serialize,
        V: Serialize,
        S: Serialize,
    {
        let buffer = common::serialize_batch(facts)?;

        // Acquire lock only for the duration of the write
        self.acquire_lock()?;

        let result = (|| {
            self.writer.write_all(&buffer)?;
            self.writer.flush()?;
            self.file.sync_all()?;
            Ok(())
        })();

        // Always release lock, even on error
        let _ = FileExt::unlock(&self.file);

        result
    }
}

pub struct FactStreamReader<E, V, S> {
    reader: BufReader<File>,
    _phantom: std::marker::PhantomData<(E, V, S)>,
}

impl<E, V, S> FactStreamReader<E, V, S> {
    /// Open a fact stream file for reading.
    ///
    /// Acquires a shared lock immediately or fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ReadError> {
        Self::open_with_timeout(path, Duration::from_secs(0))
    }

    /// Open with a timeout for acquiring the lock.
    pub fn open_with_timeout(path: impl AsRef<Path>, timeout: Duration) -> Result<Self, ReadError> {
        let file = OpenOptions::new().read(true).open(path)?;

        let start = Instant::now();
        let retry_interval = Duration::from_millis(100);

        loop {
            match FileExt::try_lock_shared(&file) {
                Ok(()) => {
                    let reader = BufReader::new(file);
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
                    std::thread::sleep(retry_interval);
                }
            }
        }
    }
}

impl<E, V, S> Iterator for FactStreamReader<E, V, S>
where
    E: DeserializeOwned,
    V: DeserializeOwned,
    S: DeserializeOwned,
{
    type Item = Result<Fact<E, V, S>, ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();

        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None, // EOF
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue; // Skip empty lines
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
