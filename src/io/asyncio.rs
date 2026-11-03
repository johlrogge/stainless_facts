use super::common;
use super::{ReadError, WriteError};
use crate::{Fact, StreamPosition};
use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};

/// Appends batches of facts to a fact stream file asynchronously.
///
/// # Locking
///
/// The same as [`FactStreamWriter`](crate::FactStreamWriter): each
/// [`write_batch`](Self::write_batch) takes an exclusive advisory file lock
/// (via `fs2`) for the duration of the write, which serializes writers to
/// the same file, including writers in other processes. Readers take no
/// lock.
///
/// The whole locked section (waiting for the lock, the write, fsync and
/// releasing the lock) runs as one task on tokio's blocking thread pool.
/// While another writer holds the lock, that task waits on its blocking
/// thread, not on the async runtime.
///
/// On Windows, the lock is taken with `LockFileEx`, which is *mandatory*
/// locking: while a write is in progress, reads of the file can block or
/// fail.
pub struct AsyncFactStreamWriter {
    /// Shared with the blocking task that does each locked write.
    file: Arc<std::fs::File>,
    lock_timeout: Duration,
}

impl AsyncFactStreamWriter {
    /// Open a fact stream file for writing.
    ///
    /// The lock is acquired only during write operations, not continuously.
    /// The lock timeout is zero: a write fails at once with
    /// [`WriteError::AlreadyLocked`] if another writer holds the lock. Use
    /// [`open_with_timeout`](Self::open_with_timeout) to wait instead.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        Self::open_with_timeout(path, Duration::from_secs(0)).await
    }

    /// Open with a timeout for acquiring the lock during writes.
    ///
    /// A write waits up to `timeout` for another writer to release the lock,
    /// then fails with [`WriteError::LockTimeout`].
    pub async fn open_with_timeout(
        path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, WriteError> {
        // Read access lets a writer check for a torn tail before appending.
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)
            .await?;

        Ok(Self {
            file: Arc::new(file.into_std().await),
            lock_timeout: timeout,
        })
    }

    /// Write a batch of facts.
    ///
    /// All facts are serialized to memory first. If serialization fails,
    /// no facts are written. The batch is then appended contiguously under
    /// the exclusive lock, and fsync ensures durability. The append is not
    /// atomic for concurrent readers (they may observe a prefix) or across
    /// a crash (a torn tail is repaired by the next writer: if the file
    /// ends in an unterminated line, a `'\n'` is written first, so the
    /// fragment becomes a single malformed line that readers report and
    /// skip).
    ///
    /// # Cancellation
    ///
    /// The batch is encoded before the first await. The locked section then
    /// runs as a task on tokio's blocking thread pool, and that task always
    /// runs to completion. If the returned future is dropped once the task
    /// has started, the write is **not** cancelled: the batch is still
    /// written in full, and the lock is released only after that. Dropping
    /// the future only discards the result.
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
        self.append_locked(buffer).await
    }

    /// The current end of the stream, taken under the exclusive lock (so no
    /// write is in progress) after repairing a torn tail. The result is a
    /// line boundary. Like [`write_batch`](Self::write_batch), it runs to
    /// completion once started, even if the future is dropped.
    pub(crate) async fn end_position(&mut self) -> Result<StreamPosition, WriteError> {
        self.append_locked(Vec::new()).await
    }

    /// Run [`common::append_locked`] on the blocking thread pool. The task
    /// owns a handle to the file and the buffer, so it runs to completion
    /// (and releases the lock) even if this future is dropped.
    async fn append_locked(&mut self, buffer: Vec<u8>) -> Result<StreamPosition, WriteError> {
        let file = Arc::clone(&self.file);
        let timeout = self.lock_timeout;
        tokio::task::spawn_blocking(move || common::append_locked(&file, timeout, buffer))
            .await
            .map_err(|e| WriteError::Io(io::Error::other(e)))?
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
///
/// # Concurrency
///
/// The same as [`FactStreamReader`](crate::FactStreamReader): the reader
/// takes no file lock and never blocks writers. A reader that races a
/// writer may observe only a prefix of an in-progress batch, but its
/// [`position`](Self::position) always stays on a line boundary. On
/// Windows, writers' `LockFileEx` locks are mandatory, so a read can block
/// or fail while a write is in progress.
pub struct AsyncFactStreamReader<E, V, S> {
    cursor: LineCursor,
    _phantom: std::marker::PhantomData<(E, V, S)>,
}

impl<E, V, S> AsyncFactStreamReader<E, V, S> {
    /// Open a fact stream file for reading from the start.
    ///
    /// Equivalent to [`open_at`](Self::open_at) with
    /// [`StreamPosition::START`].
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, ReadError> {
        Self::open_at(path, StreamPosition::START).await
    }

    /// Open a fact stream file for reading, starting at `at`.
    ///
    /// `at` should be a position returned by this stream's writer or
    /// reader. No lock is taken.
    ///
    /// # Errors
    ///
    /// [`ReadError::InvalidPosition`] if `at` is past the end of the file or
    /// is not at the start of a line.
    pub async fn open_at(path: impl AsRef<Path>, at: StreamPosition) -> Result<Self, ReadError> {
        Ok(Self {
            cursor: LineCursor::open_at(File::open(path).await?, at).await?,
            _phantom: std::marker::PhantomData,
        })
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
/// counted, only complete lines, and the position to resume from. No lock
/// is taken; a call that races a writer may return only a prefix of an
/// in-progress batch, and the returned position is on a line boundary.
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
