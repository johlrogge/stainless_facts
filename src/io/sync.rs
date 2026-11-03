// stainless_facts/src/io/sync.rs

use super::common;
use super::{ReadError, WriteError};
use crate::{Fact, StreamPosition};
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

/// Appends batches of facts to a fact stream file.
///
/// # Locking
///
/// Each [`write_batch`](Self::write_batch) takes an exclusive advisory file
/// lock (via `fs2`) for the duration of the write and releases it
/// afterwards. The lock serializes writers to the same file, including
/// writers in other processes, so their batches never interleave. Readers
/// take no lock and are never blocked by it on Unix; see
/// [`FactStreamReader`].
///
/// On Windows, the lock is taken with `LockFileEx`, which is *mandatory*
/// locking: while a write is in progress, reads of the file can block or
/// fail.
pub struct FactStreamWriter {
    file: File,
    lock_timeout: Duration,
}

impl FactStreamWriter {
    /// Open a fact stream file for writing.
    ///
    /// The lock is acquired only during write operations, not continuously.
    /// The lock timeout is zero: a write fails at once with
    /// [`WriteError::AlreadyLocked`] if another writer holds the lock. Use
    /// [`open_with_timeout`](Self::open_with_timeout) to wait instead.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        Self::open_with_timeout(path, Duration::from_secs(0))
    }

    /// Open with a timeout for acquiring the lock during writes.
    ///
    /// A write waits up to `timeout` for another writer to release the lock,
    /// then fails with [`WriteError::LockTimeout`].
    pub fn open_with_timeout(
        path: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self, WriteError> {
        // Read access lets a writer check for a torn tail before appending.
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        Ok(Self {
            file,
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
    /// Returns the position just past the last written line, taken under
    /// the write lock: the end of the stream once this batch is on disk.
    /// Reading from it (with [`FactStreamReader::open_at`]) yields exactly
    /// the facts written after this batch.
    pub fn write_batch<E, V, S>(
        &mut self,
        facts: &[Fact<E, V, S>],
    ) -> Result<StreamPosition, WriteError>
    where
        E: Serialize,
        V: Serialize,
        S: Serialize,
    {
        let buffer = common::serialize_batch(facts)?;
        common::append_locked(&self.file, self.lock_timeout, buffer)
    }

    /// The current end of the stream, taken under the exclusive lock (so no
    /// write is in progress) after repairing a torn tail. The result is a
    /// line boundary.
    pub(crate) fn end_position(&mut self) -> Result<StreamPosition, WriteError> {
        common::append_locked(&self.file, self.lock_timeout, Vec::new())
    }
}

/// Check that `at` is a line boundary of `file` and leave the file's cursor
/// there.
///
/// A position is valid if it is within the file and is either the start or
/// directly follows a `'\n'`.
fn seek_to_line_boundary(file: &mut File, at: StreamPosition) -> Result<(), ReadError> {
    let offset = at.offset();
    if offset > file.metadata()?.len() {
        return Err(ReadError::InvalidPosition(at));
    }
    if offset == 0 {
        file.seek(SeekFrom::Start(0))?;
        return Ok(());
    }
    file.seek(SeekFrom::Start(offset - 1))?;
    let mut previous = [0u8];
    file.read_exact(&mut previous)?;
    if previous[0] != b'\n' {
        return Err(ReadError::InvalidPosition(at));
    }
    Ok(())
}

/// Reads complete (newline-terminated) lines and tracks the position after
/// the last one.
///
/// A trailing line without `'\n'` may still be being written, so it is kept
/// in `buf` without advancing `pos`. The next call continues reading into
/// the same buffer. Whitespace-only lines are skipped, but `pos` still
/// advances past them. I/O errors do not advance `pos`.
struct LineCursor {
    reader: BufReader<File>,
    /// The line being read: a complete line (ending in `'\n'`) that was
    /// returned by the last call, or a partial line still waiting for its
    /// newline.
    buf: Vec<u8>,
    /// Offset just past the last complete line consumed.
    pos: u64,
}

impl LineCursor {
    /// Start reading `file` at `at`, which is checked to be a line boundary.
    fn open_at(mut file: File, at: StreamPosition) -> Result<Self, ReadError> {
        seek_to_line_boundary(&mut file, at)?;
        Ok(Self {
            reader: BufReader::new(file),
            buf: Vec::new(),
            pos: at.offset(),
        })
    }

    fn position(&self) -> StreamPosition {
        StreamPosition::from_offset(self.pos)
    }

    /// The next complete, non-blank line, including its `'\n'`.
    ///
    /// `None` means no complete line is available yet. More may become
    /// available once the writer appends, so calling again later can
    /// return `Some`.
    fn next_line(&mut self) -> Option<io::Result<&[u8]>> {
        if self.buf.last() == Some(&b'\n') {
            // Consumed by the previous call.
            self.buf.clear();
        }
        loop {
            match self.reader.read_until(b'\n', &mut self.buf) {
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

/// Reads facts from a fact stream file.
///
/// The reader yields only complete, newline-terminated lines: a trailing
/// line without `'\n'` may still be being written, so it is neither yielded
/// nor advanced past. Blank lines are skipped. A line that fails to decode
/// is yielded as `Err` and skipped, so iteration can continue past it.
///
/// The iterator is **not fused**: after it returns `None` at the end of the
/// stream, it returns `Some` again once more lines are appended. Calling
/// [`next`](Iterator::next) again later tails the stream.
///
/// [`position`](Self::position) is where to resume reading later with
/// [`open_at`](Self::open_at).
///
/// # Concurrency
///
/// The reader takes no file lock, so it never blocks writers, and writers
/// can append while it is open (it tails what they write). A reader that
/// races a writer may observe only a prefix of an in-progress batch: the
/// lines written so far. Its [`position`](Self::position) always stays on
/// a line boundary, so the rest of the batch is yielded by a later call or
/// by a reader reopened at that position.
///
/// On Windows, writers lock with `LockFileEx`, which is mandatory locking,
/// so a read can block or fail while a write is in progress; see
/// [`FactStreamWriter`].
pub struct FactStreamReader<E, V, S> {
    cursor: LineCursor,
    _phantom: std::marker::PhantomData<(E, V, S)>,
}

impl<E, V, S> FactStreamReader<E, V, S> {
    /// Open a fact stream file for reading from the start.
    ///
    /// Equivalent to [`open_at`](Self::open_at) with
    /// [`StreamPosition::START`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ReadError> {
        Self::open_at(path, StreamPosition::START)
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
    pub fn open_at(path: impl AsRef<Path>, at: StreamPosition) -> Result<Self, ReadError> {
        Ok(Self {
            cursor: LineCursor::open_at(File::open(path)?, at)?,
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
}

impl<E, V, S> Iterator for FactStreamReader<E, V, S>
where
    E: DeserializeOwned,
    V: DeserializeOwned,
    S: DeserializeOwned,
{
    type Item = Result<Fact<E, V, S>, ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.cursor.next_line()? {
            Ok(line) => Fact::decode_bytes(line).map_err(ReadError::Deserialization),
            Err(e) => Err(ReadError::Io(e)),
        })
    }
}

/// Read up to `limit` raw lines from a fact stream, starting at `at`.
///
/// This is a pass-through for relays that forward lines without decoding
/// them. Lines are returned without their trailing `'\n'` (or `"\r\n"`).
/// Blank lines are skipped and do not count toward `limit`. Only complete
/// lines are returned: a trailing line without `'\n'` may still be being
/// written, so it is left for a later call.
///
/// Returns the lines and the position to pass as `at` to read the next
/// page. When no more complete lines are available, the lines are empty and
/// the position is unchanged.
///
/// No lock is taken: the stream's writer appends each batch contiguously
/// under its exclusive lock, and only complete lines are read. As with [`FactStreamReader`],
/// a call that races a writer may return only a prefix of an in-progress
/// batch, and the returned position is always on a line boundary.
///
/// # Errors
///
/// [`ReadError::InvalidPosition`] if `at` is past the end of the file or is
/// not at the start of a line. A line that is not valid UTF-8 fails the
/// whole call with an [`io::ErrorKind::InvalidData`] error.
pub fn read_lines_at(
    path: impl AsRef<Path>,
    at: StreamPosition,
    limit: usize,
) -> Result<(Vec<String>, StreamPosition), ReadError> {
    let mut cursor = LineCursor::open_at(File::open(path)?, at)?;
    let mut lines = Vec::new();
    while lines.len() < limit {
        match cursor.next_line() {
            None => break,
            Some(line) => lines.push(common::line_to_string(line?)?),
        }
    }
    Ok((lines, cursor.position()))
}
