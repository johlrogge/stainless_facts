use super::WriteError;
use crate::{Fact, StreamPosition};
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::{Duration, Instant};

/// How long a writer waits between attempts to take a contended lock.
pub(crate) const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// An exclusive advisory lock on a fact stream file, released on drop.
///
/// Because the lock is released by `Drop`, it is also released when a write
/// fails part-way or when an async write is cancelled.
pub(crate) struct ExclusiveLock<'a>(&'a std::fs::File);

impl Drop for ExclusiveLock<'_> {
    fn drop(&mut self) {
        // Nothing useful can be done if unlocking fails; the lock is released
        // anyway when the file is closed.
        let _ = fs2::FileExt::unlock(self.0);
    }
}

/// Make one attempt to take the exclusive lock on `file`.
///
/// Returns the lock if it was taken, or `None` if another writer holds it
/// and the caller should wait [`LOCK_RETRY_INTERVAL`] and try again.
/// `started` is when the first attempt was made.
///
/// # Errors
///
/// See [`retry_or_fail`].
pub(crate) fn try_lock_exclusive(
    file: &std::fs::File,
    timeout: Duration,
    started: Instant,
) -> Result<Option<ExclusiveLock<'_>>, WriteError> {
    match fs2::FileExt::try_lock_exclusive(file) {
        Ok(()) => Ok(Some(ExclusiveLock(file))),
        Err(e) => retry_or_fail(e, timeout, started.elapsed()).map(|()| None),
    }
}

/// Take the exclusive lock on `file`, blocking the thread and retrying
/// every [`LOCK_RETRY_INTERVAL`] for up to `timeout`.
///
/// # Errors
///
/// See [`retry_or_fail`].
pub(crate) fn acquire_lock_blocking(
    file: &std::fs::File,
    timeout: Duration,
) -> Result<ExclusiveLock<'_>, WriteError> {
    let started = Instant::now();
    loop {
        if let Some(lock) = try_lock_exclusive(file, timeout, started)? {
            return Ok(lock);
        }
        std::thread::sleep(LOCK_RETRY_INTERVAL);
    }
}

/// Append `buffer` to `file` under the exclusive lock, and return the
/// length of the file afterwards.
///
/// This is the whole locked section of a write, shared by the sync writer
/// and (inside `spawn_blocking`) the async writer. It blocks the thread.
/// Under the lock, a torn tail (a file that does not end in `'\n'`) is
/// terminated first by putting a `'\n'` in front of `buffer`. If there is
/// nothing to write, no write or fsync is made. The lock is released when
/// this returns, including on error.
pub(crate) fn append_locked(
    file: &std::fs::File,
    timeout: Duration,
    mut buffer: Vec<u8>,
) -> Result<StreamPosition, WriteError> {
    let _lock = acquire_lock_blocking(file, timeout)?;
    let mut file = file;

    if ends_mid_line(file)? {
        buffer.insert(0, b'\n');
    }
    if !buffer.is_empty() {
        file.write_all(&buffer)?;
        file.sync_all()?;
    }
    Ok(StreamPosition::from_offset(file.metadata()?.len()))
}

/// Whether `file` is non-empty and does not end in `'\n'`, as when a writer
/// crashed mid-write.
fn ends_mid_line(mut file: &std::fs::File) -> std::io::Result<bool> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(len - 1))?;
    let mut last = [0u8];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

/// Classify a failed lock attempt after waiting `waited` so far.
///
/// `Ok(())` means the lock is held by another writer and the caller should
/// retry.
///
/// # Errors
///
/// - [`WriteError::AlreadyLocked`] if the lock is contended and `timeout`
///   is zero.
/// - [`WriteError::LockTimeout`] if the lock is contended and `waited` has
///   reached `timeout`.
/// - [`WriteError::Io`] for any error other than contention, which retrying
///   cannot fix.
pub(crate) fn retry_or_fail(
    e: std::io::Error,
    timeout: Duration,
    waited: Duration,
) -> Result<(), WriteError> {
    if e.kind() != fs2::lock_contended_error().kind() {
        Err(WriteError::Io(e))
    } else if timeout.is_zero() {
        Err(WriteError::AlreadyLocked)
    } else if waited >= timeout {
        Err(WriteError::LockTimeout(timeout))
    } else {
        Ok(())
    }
}

/// Serialize a batch of facts to a buffer.
///
/// Each fact is encoded with the shared line codec ([`Fact::to_line`] plus
/// `'\n'`), so the output is byte-identical to the public codec by
/// construction. Newlines within fact values are escaped by JSON.
pub(crate) fn serialize_batch<E, V, S>(
    facts: &[Fact<E, V, S>],
) -> Result<Vec<u8>, serde_json::Error>
where
    E: Serialize,
    V: Serialize,
    S: Serialize,
{
    let mut buffer = Vec::new();

    for fact in facts {
        fact.encode_into(&mut buffer)?;
    }

    Ok(buffer)
}

/// Convert a complete line, as returned by a line cursor, to a `String`
/// without its trailing `'\n'` or `"\r\n"` (as [`Fact::from_line`] accepts
/// either).
pub(crate) fn line_to_string(line: &[u8]) -> std::io::Result<String> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8(line.to_vec())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Operation;
    use tempfile::NamedTempFile;

    fn contended() -> std::io::Error {
        fs2::lock_contended_error()
    }

    #[test]
    fn contention_with_zero_timeout_is_already_locked() {
        let result = retry_or_fail(contended(), Duration::ZERO, Duration::ZERO);
        assert!(matches!(result, Err(WriteError::AlreadyLocked)));
    }

    #[test]
    fn contention_within_the_timeout_is_retried() {
        let timeout = Duration::from_secs(1);
        assert!(retry_or_fail(contended(), timeout, Duration::from_millis(10)).is_ok());
    }

    #[test]
    fn contention_past_the_timeout_is_lock_timeout() {
        let timeout = Duration::from_millis(100);
        let result = retry_or_fail(contended(), timeout, timeout);
        assert!(matches!(result, Err(WriteError::LockTimeout(t)) if t == timeout));
    }

    #[test]
    fn errors_other_than_contention_are_io_errors() {
        for timeout in [Duration::ZERO, Duration::from_secs(1)] {
            let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
            let result = retry_or_fail(e, timeout, Duration::ZERO);
            assert!(
                matches!(&result, Err(WriteError::Io(e)) if e.kind() == std::io::ErrorKind::PermissionDenied),
                "expected Io(PermissionDenied), got {result:?}"
            );
        }
    }

    #[test]
    fn dropping_the_lock_releases_it() {
        let temp = NamedTempFile::new().unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(temp.path())
            .unwrap();
        let other = std::fs::OpenOptions::new()
            .write(true)
            .open(temp.path())
            .unwrap();

        let lock = try_lock_exclusive(&file, Duration::ZERO, Instant::now())
            .unwrap()
            .expect("the lock is free");
        assert!(matches!(
            try_lock_exclusive(&other, Duration::ZERO, Instant::now()),
            Err(WriteError::AlreadyLocked)
        ));

        drop(lock);
        assert!(try_lock_exclusive(&other, Duration::ZERO, Instant::now())
            .unwrap()
            .is_some());
    }

    #[test]
    fn newlines_in_values_are_escaped() {
        let fact = Fact::new(
            "track1".to_string(),
            "Line 1\nLine 2\nLine 3".to_string(),
            "2024-01-15T10:00:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert,
        );

        let buffer = serialize_batch(&[fact]).unwrap();
        let text = String::from_utf8(buffer).unwrap();

        // Should be exactly one newline (the delimiter)
        assert_eq!(text.matches('\n').count(), 1);
        assert!(text.contains("\\n"));
    }
}
