use super::{common, ReadError, WriteError};
use crate::{Fact, StreamPosition};
use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};

pub struct AsyncFactStreamWriter {
    sync_file: std::fs::File, // For locking
    file: File,
    lock_timeout: Duration,
}

impl AsyncFactStreamWriter {
    /// Open a fact stream file for writing.
    ///
    /// The lock is acquired only during write operations, not continuously.
    /// This allows readers to access the file between writes.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        Self::open_with_timeout(path, Duration::from_secs(0)).await
    }

    /// Open with a timeout for acquiring the lock during writes.
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

        // Convert to async file
        let file = File::from_std(sync_file.try_clone()?);

        Ok(Self {
            sync_file,
            file,
            lock_timeout: timeout,
        })
    }

    /// Acquire exclusive lock with configured timeout
    async fn acquire_lock(&self) -> Result<(), WriteError> {
        let start = Instant::now();
        let retry_interval = Duration::from_millis(100);

        loop {
            match self.sync_file.try_lock_exclusive() {
                Ok(()) => return Ok(()),
                Err(_) if self.lock_timeout.is_zero() => {
                    return Err(WriteError::AlreadyLocked);
                }
                Err(_) if start.elapsed() >= self.lock_timeout => {
                    return Err(WriteError::LockTimeout(self.lock_timeout));
                }
                Err(_) => {
                    tokio::time::sleep(retry_interval).await;
                }
            }
        }
    }

    /// Write a batch of facts atomically.
    ///
    /// Acquires exclusive lock, writes all facts, then releases lock.
    /// All facts are serialized to memory first. If serialization fails,
    /// no facts are written. The batch is appended with a single write, and
    /// fsync ensures durability.
    ///
    /// Returns the position just past the last written line, taken under
    /// the write lock: the end of the stream once this batch is on disk.
    /// Reading from it (with [`AsyncFactStreamReader::open_at`]) yields
    /// exactly the facts written after this batch.
    pub async fn write_batch<E, V, S>(
        &mut self,
        facts: &[Fact<E, V, S>],
    ) -> Result<StreamPosition, WriteError>
    where
        E: Serialize,
        V: Serialize,
        S: Serialize,
    {
        let buffer = common::serialize_batch(facts)?;

        // Acquire lock only for the duration of the write
        self.acquire_lock().await?;

        let result = async {
            self.file.write_all(&buffer).await?;
            // tokio's `File` completes writes in the background; flush waits
            // for this one to land before syncing.
            self.file.flush().await?;
            self.file.sync_all().await?;
            Ok(StreamPosition::from_offset(
                self.file.metadata().await?.len(),
            ))
        }
        .await;

        // Always release lock, even on error
        let _ = FileExt::unlock(&self.sync_file);

        result
    }
}

/// Check that `at` is a line boundary of `file` and leave the file's cursor
/// there. See the sync reader for the rules.
async fn seek_to_line_boundary(file: &mut File, at: StreamPosition) -> Result<(), ReadError> {
    let offset = at.offset();
    if offset > file.metadata().await?.len() {
        return Err(ReadError::InvalidPosition(at));
    }
    if offset == 0 {
        file.seek(SeekFrom::Start(0)).await?;
        return Ok(());
    }
    file.seek(SeekFrom::Start(offset - 1)).await?;
    let mut previous = [0u8];
    file.read_exact(&mut previous).await?;
    if previous[0] != b'\n' {
        return Err(ReadError::InvalidPosition(at));
    }
    Ok(())
}

/// Reads complete (newline-terminated) lines and tracks the position after
/// the last one. The tokio twin of the sync `LineCursor`.
///
/// A trailing line without `'\n'` is kept in `buf` without advancing `pos`.
/// Because the partial line lives in `self` rather than in the future,
/// [`next_line`](Self::next_line) is cancel-safe.
struct LineCursor {
    reader: BufReader<File>,
    buf: Vec<u8>,
    pos: u64,
}

impl LineCursor {
    async fn open_at(mut file: File, at: StreamPosition) -> Result<Self, ReadError> {
        seek_to_line_boundary(&mut file, at).await?;
        Ok(Self {
            reader: BufReader::new(file),
            buf: Vec::new(),
            pos: at.offset(),
        })
    }

    fn position(&self) -> StreamPosition {
        StreamPosition::from_offset(self.pos)
    }

    /// The next complete, non-blank line, including its `'\n'`, or `None`
    /// if no complete line is available yet.
    async fn next_line(&mut self) -> Option<io::Result<&[u8]>> {
        if self.buf.last() == Some(&b'\n') {
            // Consumed by the previous call.
            self.buf.clear();
        }
        loop {
            match self.reader.read_until(b'\n', &mut self.buf).await {
                Err(e) => return Some(Err(e)),
                // EOF, or a partial line still waiting for its newline.
                Ok(_) if self.buf.last() != Some(&b'\n') => return None,
                Ok(_) => {
                    self.pos += self.buf.len() as u64;
                    if self.buf.trim_ascii().is_empty() {
                        self.buf.clear();
                        continue;
                    }
                    return Some(Ok(&self.buf));
                }
            }
        }
    }
}

/// Reads facts from a fact stream file asynchronously.
///
/// The reader yields only complete, newline-terminated lines: a trailing
/// line without `'\n'` may still be being written, so it is neither yielded
/// nor advanced past. Blank lines are skipped. A line that fails to decode
/// is yielded as `Err` and skipped, so reading can continue past it.
///
/// The reader is **not fused**: after [`next`](Self::next) returns `None`
/// at the end of the stream, it returns `Some` again once more lines are
/// appended. Calling `next` again later tails the stream. `next` is
/// cancel-safe.
///
/// [`position`](Self::position) is where to resume reading later with
/// [`open_at`](Self::open_at).
pub struct AsyncFactStreamReader<E, V, S> {
    cursor: LineCursor,
    _phantom: std::marker::PhantomData<(E, V, S)>,
}

impl<E, V, S> AsyncFactStreamReader<E, V, S> {
    /// Open a fact stream file for reading from the start.
    ///
    /// Acquires a shared lock immediately or fails. Equivalent to
    /// [`open_at`](Self::open_at) with [`StreamPosition::START`].
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, ReadError> {
        Self::open_at(path, StreamPosition::START).await
    }

    /// Open a fact stream file for reading, starting at `at`.
    ///
    /// `at` should be a position returned by this stream's writer or
    /// reader. Acquires a shared lock immediately or fails.
    ///
    /// # Errors
    ///
    /// [`ReadError::InvalidPosition`] if `at` is past the end of the file or
    /// is not at the start of a line.
    pub async fn open_at(path: impl AsRef<Path>, at: StreamPosition) -> Result<Self, ReadError> {
        Self::open_at_with_timeout(path, at, Duration::from_secs(0)).await
    }

    /// Open with a timeout for acquiring the lock.
    pub async fn open_with_timeout(
        path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, ReadError> {
        Self::open_at_with_timeout(path, StreamPosition::START, timeout).await
    }

    async fn open_at_with_timeout(
        path: impl AsRef<Path>,
        at: StreamPosition,
        timeout: Duration,
    ) -> Result<Self, ReadError> {
        let path = path.as_ref();

        let sync_file = std::fs::OpenOptions::new().read(true).open(path)?;

        let start = Instant::now();
        let retry_interval = Duration::from_millis(100);

        loop {
            match FileExt::try_lock_shared(&sync_file) {
                Ok(()) => {
                    let cursor = LineCursor::open_at(File::from_std(sync_file), at).await?;
                    return Ok(Self {
                        cursor,
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

    /// The position just past the last complete line consumed.
    ///
    /// Blank lines and lines that failed to decode count as consumed. At
    /// the end of the stream this is the length of the file, unless the
    /// file ends in an unterminated line.
    pub fn position(&self) -> StreamPosition {
        self.cursor.position()
    }

    /// Read the next fact, or `None` if no complete line is available yet.
    pub async fn next(&mut self) -> Option<Result<Fact<E, V, S>, ReadError>>
    where
        E: DeserializeOwned,
        V: DeserializeOwned,
        S: DeserializeOwned,
    {
        Some(match self.cursor.next_line().await? {
            Ok(line) => Fact::decode_bytes(line).map_err(ReadError::Deserialization),
            Err(e) => Err(ReadError::Io(e)),
        })
    }
}

/// Read up to `limit` raw lines from a fact stream, starting at `at`.
///
/// The async twin of [`read_lines_at`](crate::read_lines_at), with the same
/// semantics: lines without their `'\n'`, blank lines skipped and not
/// counted, only complete lines, and the position to resume from.
///
/// # Errors
///
/// [`ReadError::InvalidPosition`] if `at` is past the end of the file or is
/// not at the start of a line. A line that is not valid UTF-8 fails the
/// whole call with an [`io::ErrorKind::InvalidData`] error.
pub async fn read_lines_at(
    path: impl AsRef<Path>,
    at: StreamPosition,
    limit: usize,
) -> Result<(Vec<String>, StreamPosition), ReadError> {
    let mut cursor = LineCursor::open_at(File::open(path).await?, at).await?;
    let mut lines = Vec::new();
    while lines.len() < limit {
        match cursor.next_line().await {
            None => break,
            Some(line) => lines.push(common::line_to_string(line?)?),
        }
    }
    Ok((lines, cursor.position()))
}
