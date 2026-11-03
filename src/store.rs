// stainless-facts enhancement: FactStore with iter_from()
// =======================================================
//
// Add to: src/store.rs (new file)

use crate::{
    io::{FactStreamReader, FactStreamWriter, ReadError, WriteError},
    Fact, StreamPosition,
};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    io::BufRead,
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;

/// How long a store's append waits for another writer's file lock before
/// failing with [`WriteError::LockTimeout`], unless changed with
/// [`FactStore::with_lock_timeout`] (or the async store's equivalent).
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Write error: {0}")]
    Write(#[from] WriteError),

    #[error("Read error: {0}")]
    Read(#[from] ReadError),

    #[error("Timestamp ordering violation: new fact at {new} is before latest {latest}")]
    TimestampOrdering {
        new: DateTime<Utc>,
        latest: DateTime<Utc>,
    },
}

/// A thread-safe fact store that maintains timestamp ordering.
///
/// The FactStore provides:
/// - Thread-safe appending of facts with timestamp validation
/// - Resumable reads from a [`StreamPosition`] with [`read_at`](Self::read_at)
/// - Iteration from any timestamp
///
/// # Concurrency
///
/// Appends through one store are serialized by an in-process mutex held
/// across the timestamp check, the write and the update of the cached
/// latest timestamp. Two threads therefore cannot both pass the check and
/// then write out of order. Each write also takes the stream's exclusive
/// file lock (see [`FactStreamWriter`]), which serializes it with writers in
/// other processes; an append waits up to the store's lock timeout
/// ([`DEFAULT_LOCK_TIMEOUT`] unless set with
/// [`with_lock_timeout`](Self::with_lock_timeout)) for that lock. The
/// timestamp check uses this store's cache only, so it does not order
/// appends made through other stores or processes.
///
/// Readers ([`read_at`](Self::read_at), [`iter`](Self::iter)) take no
/// lock: they never block appends, and appends never block them (except on
/// Windows; see [`FactStreamReader`]).
///
/// # Examples
///
/// ```rust
/// use stainless_facts::store::FactStore;
/// use stainless_facts::{Fact, Operation};
/// use serde::{Serialize, Deserialize};
/// use chrono::Utc;
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// #[serde(tag = "t", content = "v")]
/// enum MyValue {
///     Bpm(u16),
/// }
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let store: FactStore<String, MyValue, String> =
///     FactStore::open_or_create("facts.stream")?;
///
/// // Append a fact
/// let fact = Fact::new(
///     "track1".to_string(),
///     MyValue::Bpm(128),
///     Utc::now(),
///     "analyzer".to_string(),
///     Operation::Assert,
/// );
/// let position = store.append(fact)?;
///
/// // Later: read only the facts appended after `position`
/// for fact in store.read_at(position)? {
///     println!("{:?}", fact?);
/// }
///
/// // Iterate from a timestamp
/// let since = "2024-01-15T00:00:00Z".parse()?;
/// for fact in store.iter_from(since) {
///     let fact = fact?;
///     println!("{:?}", fact);
/// }
/// # Ok(())
/// # }
/// ```
pub struct FactStore<E, V, S> {
    path: PathBuf,
    /// Latest timestamp, cached for quick access. The guard is held for the
    /// whole append, so it also serializes appends.
    latest_timestamp: Mutex<Option<DateTime<Utc>>>,
    lock_timeout: Duration,
    _phantom: std::marker::PhantomData<(E, V, S)>,
}

impl<E, V, S> FactStore<E, V, S>
where
    E: Serialize + DeserializeOwned + Clone,
    V: Serialize + DeserializeOwned + Clone,
    S: Serialize + DeserializeOwned + Clone,
{
    /// Open an existing fact store or create a new one.
    ///
    /// An existing file is scanned once for its latest timestamp. Lines
    /// that fail to decode are skipped by that scan, so they do not prevent
    /// opening the store; [`iter`](Self::iter) reports them as errors.
    pub fn open_or_create(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Read latest timestamp if file exists
        let latest_timestamp = if path.exists() {
            Self::read_latest_timestamp(&path)?
        } else {
            None
        };

        Ok(Self {
            path,
            latest_timestamp: Mutex::new(latest_timestamp),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
            _phantom: std::marker::PhantomData,
        })
    }

    /// Set how long an append waits for another writer's file lock before
    /// failing with [`WriteError::LockTimeout`].
    ///
    /// The default is [`DEFAULT_LOCK_TIMEOUT`]. With [`Duration::ZERO`], an
    /// append fails at once with [`WriteError::AlreadyLocked`] if the lock
    /// is held.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// How long an append waits for another writer's file lock.
    pub fn lock_timeout(&self) -> Duration {
        self.lock_timeout
    }

    /// Append a single fact, enforcing timestamp ordering.
    ///
    /// Returns the position just past the written fact; see
    /// [`append_batch`](Self::append_batch).
    pub fn append(&self, fact: Fact<E, V, S>) -> Result<StreamPosition, StoreError> {
        self.append_batch(&[fact])
    }

    /// Append a batch of facts, enforcing timestamp ordering.
    ///
    /// If any fact violates timestamp ordering, the entire batch is
    /// rejected. Otherwise the facts are appended contiguously under the
    /// exclusive lock; this is not atomic for concurrent readers (they may
    /// observe a prefix) or across a crash (a torn tail is repaired by the
    /// next writer). See [`FactStreamWriter::write_batch`].
    ///
    /// Returns the position just past the batch, as returned by
    /// [`FactStreamWriter::write_batch`]. Pass it to
    /// [`read_at`](Self::read_at) to read only what was appended later. An
    /// empty batch writes no facts and returns the current end of the
    /// stream, taken under the exclusive lock (waiting for it like any
    /// append) after repairing a torn tail, so it is always a valid
    /// position for [`read_at`](Self::read_at). If the store's file does
    /// not exist yet, an empty batch returns [`StreamPosition::START`]
    /// without creating it.
    pub fn append_batch(&self, facts: &[Fact<E, V, S>]) -> Result<StreamPosition, StoreError> {
        // Held across the check, the write and the cache update, so that
        // concurrent appends cannot pass the check and then write out of
        // order.
        let mut latest = self.latest_timestamp.lock();

        if facts.is_empty() {
            // The end is taken under the file lock, after repairing a torn
            // tail, so it is a line boundary. A missing file is not created.
            return match std::fs::metadata(&self.path) {
                Ok(_) => {
                    let mut writer =
                        FactStreamWriter::open_with_timeout(&self.path, self.lock_timeout)?;
                    Ok(writer.end_position()?)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StreamPosition::START),
                Err(e) => Err(e.into()),
            };
        }

        if let Some(latest_ts) = *latest {
            for fact in facts {
                if fact.timestamp() < &latest_ts {
                    return Err(StoreError::TimestampOrdering {
                        new: *fact.timestamp(),
                        latest: latest_ts,
                    });
                }
            }
        }

        // The writer takes the exclusive file lock for the write, waiting up
        // to `lock_timeout` for writers in other processes.
        let mut writer = FactStreamWriter::open_with_timeout(&self.path, self.lock_timeout)?;
        let position = writer.write_batch(facts)?;

        if let Some(last_fact) = facts.last() {
            *latest = Some(*last_fact.timestamp());
        }

        Ok(position)
    }

    /// Read the facts after position `at`.
    ///
    /// `at` is a position returned by [`append`](Self::append),
    /// [`append_batch`](Self::append_batch) or a reader's
    /// [`position`](FactStreamReader::position), or
    /// [`StreamPosition::START`]. This is the way to sync incrementally:
    /// store the reader's position when you stop, and resume from it later.
    /// Every fact is delivered exactly once, including facts in the same
    /// batch or with the same timestamp.
    ///
    /// The reader tails the stream; see [`FactStreamReader`]. If the store's
    /// file does not exist yet, it is created empty.
    ///
    /// # Errors
    ///
    /// [`StoreError::Read`] with [`ReadError::InvalidPosition`] if `at` is
    /// past the end of the stream or not at the start of a line.
    pub fn read_at(&self, at: StreamPosition) -> Result<FactStreamReader<E, V, S>, StoreError> {
        // Make sure the file exists, without touching its contents.
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        Ok(FactStreamReader::open_at(&self.path, at)?)
    }

    /// Get the latest timestamp in the store.
    ///
    /// Waits for an append in progress through this store to finish. That
    /// append may itself be waiting for another writer's file lock, so this
    /// can take up to the store's [`lock_timeout`](Self::lock_timeout).
    pub fn latest_timestamp(&self) -> Option<DateTime<Utc>> {
        *self.latest_timestamp.lock()
    }

    /// Iterate over all facts in the store.
    ///
    /// Each item is a `Result`: a line that fails to decode is yielded as
    /// an `Err` and iteration continues past it. See [`FactIterator`] for
    /// the error and end-of-stream behaviour.
    pub fn iter(&self) -> FactIterator<E, V, S> {
        self.iter_from(DateTime::<Utc>::MIN_UTC)
    }

    /// Iterate over facts starting from a specific timestamp.
    ///
    /// Yields facts from the first one at or after `since`. This is a
    /// time-travel query, not a sync cursor: a whole batch shares one
    /// timestamp, so resuming from the last timestamp seen re-delivers it.
    /// To sync incrementally, keep a [`StreamPosition`] and use
    /// [`read_at`](Self::read_at) instead.
    ///
    /// Items are `Result`s, as for [`iter`](Self::iter). A line that fails
    /// to decode is yielded as an `Err` even before the start point, since
    /// its timestamp is unknown.
    ///
    /// # Performance
    ///
    /// Performs a linear scan from the start of the stream.
    pub fn iter_from(&self, since: DateTime<Utc>) -> FactIterator<E, V, S> {
        FactIterator::new(&self.path, since)
    }

    /// Read the latest timestamp from the file without caching.
    ///
    /// Unlike [`iter`](Self::iter), this skips lines that fail to decode
    /// rather than reporting them, so a malformed line does not prevent
    /// opening the store. I/O errors still fail.
    fn read_latest_timestamp(path: &Path) -> Result<Option<DateTime<Utc>>, StoreError> {
        let file = std::fs::File::open(path)?;
        let mut reader = std::io::BufReader::new(file);

        let mut last_timestamp = None;
        let mut line = String::new();

        // Read through file, keeping track of last timestamp
        // This is O(n) but only done once at startup
        while reader.read_line(&mut line).map_err(ReadError::from)? > 0 {
            if let Ok(fact) = Fact::<E, V, S>::from_line(&line) {
                last_timestamp = Some(*fact.timestamp());
            }
            line.clear();
        }

        Ok(last_timestamp)
    }
}

/// Tracks the start point of [`FactStore::iter_from`] (and its async twin):
/// facts are skipped until the first one at or after `since`, and passed
/// through from then on. Errors always pass through.
pub(crate) struct SinceFilter {
    since: DateTime<Utc>,
    found_starting_point: bool,
}

impl SinceFilter {
    pub(crate) fn new(since: DateTime<Utc>) -> Self {
        Self {
            since,
            found_starting_point: false,
        }
    }

    /// Whether `fact` should be yielded.
    pub(crate) fn admits<E, V, S>(&mut self, fact: &Fact<E, V, S>) -> bool {
        if !self.found_starting_point && fact.timestamp() >= &self.since {
            self.found_starting_point = true;
        }
        self.found_starting_point
    }
}

/// Iterator over the facts in a [`FactStore`], returned by
/// [`FactStore::iter`] and [`FactStore::iter_from`].
///
/// Lazily reads facts from disk with a [`FactStreamReader`], yielding the
/// facts from the first one at or after the starting timestamp.
///
/// # Errors
///
/// - A line that fails to decode is yielded as
///   `Err(`[`ReadError::Deserialization`]`)` and skipped, so iteration can
///   continue past it. Such a line is reported even before the starting
///   timestamp is reached, since its timestamp is unknown.
/// - After an I/O error (`Err(`[`ReadError::Io`]`)`), the iterator returns
///   `None` from then on, so a persistent error cannot loop forever.
/// - If the store's file does not exist, the iterator is empty.
/// - Any other error opening the file is yielded as the first item, and the
///   iterator then returns `None`.
///
/// # Tailing
///
/// Like [`FactStreamReader`], the iterator is **not fused**: after it
/// returns `None` at the end of the stream, it returns `Some` again once
/// more facts are appended. To read a snapshot, stop at the first `None`
/// (as a `for` loop does). This does not apply after an I/O error or when
/// the file was missing when the iterator was created: then it stays empty.
pub struct FactIterator<E, V, S> {
    /// `None` once iteration has ended for good (missing file, or after an
    /// I/O error).
    reader: Option<FactStreamReader<E, V, S>>,
    /// An error opening the file, yielded as the first item.
    open_error: Option<ReadError>,
    filter: SinceFilter,
}

impl<E, V, S> FactIterator<E, V, S> {
    fn new(path: &Path, since: DateTime<Utc>) -> Self {
        let (reader, open_error) = match FactStreamReader::open(path) {
            Ok(reader) => (Some(reader), None),
            Err(ReadError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => (None, None),
            Err(e) => (None, Some(e)),
        };
        Self {
            reader,
            open_error,
            filter: SinceFilter::new(since),
        }
    }
}

impl<E, V, S> Iterator for FactIterator<E, V, S>
where
    E: DeserializeOwned,
    V: DeserializeOwned,
    S: DeserializeOwned,
{
    type Item = Result<Fact<E, V, S>, ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(e) = self.open_error.take() {
            return Some(Err(e));
        }
        loop {
            match self.reader.as_mut()?.next()? {
                Ok(fact) if self.filter.admits(&fact) => return Some(Ok(fact)),
                Ok(_) => continue,
                Err(e @ ReadError::Io(_)) => {
                    self.reader = None;
                    return Some(Err(e));
                }
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{assert_fact_value_format, Fact, Operation};
    use serde::{Deserialize, Serialize};
    use tempfile::NamedTempFile;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "t", content = "v")]
    enum TestValue {
        Count(u32),
    }

    // Validate format in a test
    #[test]
    fn test_value_format() {
        assert_fact_value_format!(TestValue::Count(42));
    }

    fn create_test_facts() -> Vec<Fact<String, TestValue, String>> {
        vec![
            Fact::new(
                "item1".to_string(),
                TestValue::Count(1),
                "2024-01-15T10:00:00Z".parse().unwrap(),
                "source1".to_string(),
                Operation::Assert,
            ),
            Fact::new(
                "item2".to_string(),
                TestValue::Count(2),
                "2024-01-15T10:01:00Z".parse().unwrap(),
                "source1".to_string(),
                Operation::Assert,
            ),
            Fact::new(
                "item3".to_string(),
                TestValue::Count(3),
                "2024-01-15T10:02:00Z".parse().unwrap(),
                "source1".to_string(),
                Operation::Assert,
            ),
        ]
    }

    #[test]
    fn test_open_or_create() {
        let temp = NamedTempFile::new().unwrap();
        let store = FactStore::<String, TestValue, String>::open_or_create(temp.path()).unwrap();

        assert_eq!(store.latest_timestamp(), None);
    }

    #[test]
    fn test_append_and_latest_timestamp() {
        let temp = NamedTempFile::new().unwrap();
        let store = FactStore::open_or_create(temp.path()).unwrap();

        let facts = create_test_facts();

        store.append(facts[0].clone()).unwrap();
        assert_eq!(store.latest_timestamp(), Some(*facts[0].timestamp()));

        store.append(facts[1].clone()).unwrap();
        assert_eq!(store.latest_timestamp(), Some(*facts[1].timestamp()));
    }

    #[test]
    fn test_timestamp_ordering_enforced() {
        let temp = NamedTempFile::new().unwrap();
        let store = FactStore::open_or_create(temp.path()).unwrap();

        let facts = create_test_facts();

        // Write fact with later timestamp first
        store.append(facts[2].clone()).unwrap();

        // Try to write fact with earlier timestamp - should fail
        let result = store.append(facts[0].clone());
        assert!(matches!(result, Err(StoreError::TimestampOrdering { .. })));
    }

    #[test]
    fn test_iter_all() {
        let temp = NamedTempFile::new().unwrap();
        let store = FactStore::open_or_create(temp.path()).unwrap();

        let facts = create_test_facts();
        store.append_batch(&facts).unwrap();

        let read_facts = collect_ok(store.iter());
        assert_eq!(read_facts.len(), 3);
        assert_eq!(read_facts, facts);
    }

    #[test]
    fn test_iter_from_middle() {
        let temp = NamedTempFile::new().unwrap();
        let store = FactStore::open_or_create(temp.path()).unwrap();

        let facts = create_test_facts();
        store.append_batch(&facts).unwrap();

        // Start from second fact's timestamp
        let since = *facts[1].timestamp();
        let read_facts = collect_ok(store.iter_from(since));

        assert_eq!(read_facts.len(), 2);
        assert_eq!(read_facts[0], facts[1]);
        assert_eq!(read_facts[1], facts[2]);
    }

    #[test]
    fn test_iter_from_future() {
        let temp = NamedTempFile::new().unwrap();
        let store = FactStore::open_or_create(temp.path()).unwrap();

        let facts = create_test_facts();
        store.append_batch(&facts).unwrap();

        // Start from timestamp after all facts
        let since = "2024-01-15T11:00:00Z".parse().unwrap();
        let read_facts = collect_ok(store.iter_from(since));

        assert_eq!(read_facts.len(), 0);
    }

    #[test]
    fn test_iter_from_past() {
        let temp = NamedTempFile::new().unwrap();
        let store = FactStore::open_or_create(temp.path()).unwrap();

        let facts = create_test_facts();
        store.append_batch(&facts).unwrap();

        // Start from timestamp before all facts
        let since = "2024-01-15T09:00:00Z".parse().unwrap();
        let read_facts = collect_ok(store.iter_from(since));

        assert_eq!(read_facts.len(), 3);
        assert_eq!(read_facts, facts);
    }

    #[test]
    fn test_reopen_and_append() {
        let temp = NamedTempFile::new().unwrap();
        let facts = create_test_facts();

        // Write first fact
        {
            let store: FactStore<String, TestValue, String> =
                FactStore::open_or_create(temp.path()).unwrap();
            store.append(facts[0].clone()).unwrap();
        }

        // Reopen and append second fact
        {
            let store: FactStore<String, TestValue, String> =
                FactStore::open_or_create(temp.path()).unwrap();

            // Latest timestamp should be restored
            assert_eq!(store.latest_timestamp(), Some(*facts[0].timestamp()));

            // Should be able to append next fact
            store.append(facts[1].clone()).unwrap();
        }

        // Verify both facts are present
        {
            let store: FactStore<String, TestValue, String> =
                FactStore::open_or_create(temp.path()).unwrap();
            let read_facts = collect_ok(store.iter());
            assert_eq!(read_facts.len(), 2);
        }
    }

    #[test]
    fn test_append_returns_positions_and_read_at_resumes() {
        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();
        let facts = create_test_facts();

        let after_first = store.append(facts[0].clone()).unwrap();
        let after_rest = store.append_batch(&facts[1..]).unwrap();
        assert!(after_first < after_rest);
        assert_eq!(
            after_rest,
            StreamPosition::from_offset(std::fs::metadata(temp.path()).unwrap().len())
        );

        let resumed: Vec<_> = store
            .read_at(after_first)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(resumed, facts[1..]);

        let mut reader = store.read_at(StreamPosition::START).unwrap();
        let all: Vec<_> = reader.by_ref().collect::<Result<_, _>>().unwrap();
        assert_eq!(all, facts);
        assert_eq!(reader.position(), after_rest);

        assert!(store.read_at(after_rest).unwrap().next().is_none());
    }

    #[test]
    fn test_empty_append_batch_returns_current_position() {
        let dir = tempfile::tempdir().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(dir.path().join("facts.stream")).unwrap();

        assert_eq!(store.append_batch(&[]).unwrap(), StreamPosition::START);
        let after = store.append_batch(&create_test_facts()).unwrap();
        assert_eq!(store.append_batch(&[]).unwrap(), after);
    }

    #[test]
    fn test_empty_append_batch_on_torn_tail_returns_a_readable_position() {
        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();
        let a = count_fact(1, at("10:00:00"));
        store.append(a.clone()).unwrap();
        {
            // A crashed writer left half a line behind.
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(temp.path())
                .unwrap();
            file.write_all(br#"{"e":"torn"#).unwrap();
        }

        let end = store.append_batch(&[]).unwrap();
        assert_eq!(
            end,
            StreamPosition::from_offset(std::fs::metadata(temp.path()).unwrap().len())
        );
        assert!(store.read_at(end).unwrap().next().is_none());

        let b = count_fact(2, at("10:01:00"));
        store.append(b.clone()).unwrap();
        let after: Vec<_> = store
            .read_at(end)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(after, vec![b]);
    }

    #[test]
    fn test_empty_append_batch_on_missing_file_does_not_create_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("facts.stream");
        let store: FactStore<String, TestValue, String> = FactStore::open_or_create(&path).unwrap();

        assert_eq!(store.append_batch(&[]).unwrap(), StreamPosition::START);
        assert!(!path.exists());
    }

    #[test]
    fn test_read_at_on_store_without_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(dir.path().join("facts.stream")).unwrap();

        assert!(store
            .read_at(StreamPosition::START)
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn test_read_at_invalid_position_is_a_read_error() {
        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();
        store.append_batch(&create_test_facts()).unwrap();

        let result = store.read_at(StreamPosition::from_offset(1));
        assert!(matches!(
            result,
            Err(StoreError::Read(ReadError::InvalidPosition(_)))
        ));
    }

    #[test]
    fn test_empty_store_iter() {
        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();

        let facts = collect_ok(store.iter());
        assert_eq!(facts.len(), 0);
    }

    type TestFact = Fact<String, TestValue, String>;

    fn collect_ok(iter: FactIterator<String, TestValue, String>) -> Vec<TestFact> {
        iter.collect::<Result<_, _>>().unwrap()
    }

    /// Each item as `Some(fact)`, or `None` for a line that failed to
    /// decode. Any other error fails the test.
    fn outcomes(iter: FactIterator<String, TestValue, String>) -> Vec<Option<TestFact>> {
        iter.map(|item| match item {
            Ok(fact) => Some(fact),
            Err(ReadError::Deserialization(_)) => None,
            Err(e) => panic!("expected a decode error, got {e:?}"),
        })
        .collect()
    }

    fn append_garbage_line(path: &Path) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(b"this is not a fact\n").unwrap();
    }

    fn at(time: &str) -> DateTime<Utc> {
        format!("2024-01-15T{time}Z").parse().unwrap()
    }

    #[test]
    fn test_iter_yields_err_for_malformed_line_and_continues() {
        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();
        let a = count_fact(1, at("10:00:00"));
        let b = count_fact(2, at("10:01:00"));
        let c = count_fact(3, at("10:02:00"));

        store.append(a.clone()).unwrap();
        append_garbage_line(temp.path());
        store.append_batch(&[b.clone(), c.clone()]).unwrap();

        assert_eq!(
            outcomes(store.iter()),
            vec![Some(a), None, Some(b), Some(c)]
        );
    }

    #[test]
    fn test_iter_from_yields_err_for_malformed_line_and_continues() {
        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();
        let early = count_fact(0, at("09:00:00"));
        let a = count_fact(1, at("10:00:00"));
        let b = count_fact(2, at("10:01:00"));
        let c = count_fact(3, at("10:02:00"));

        store.append_batch(&[early, a.clone()]).unwrap();
        append_garbage_line(temp.path());
        store.append_batch(&[b.clone(), c.clone()]).unwrap();

        // The fact before `since` is filtered out; the malformed line after
        // the start point is still reported.
        assert_eq!(
            outcomes(store.iter_from(*a.timestamp())),
            vec![Some(a), None, Some(b.clone()), Some(c.clone())]
        );

        // A malformed line before the start point is reported too: its
        // timestamp is unknown, so it cannot be filtered out.
        assert_eq!(
            outcomes(store.iter_from(*b.timestamp())),
            vec![None, Some(b), Some(c)]
        );
    }

    #[test]
    fn test_iter_on_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(dir.path().join("facts.stream")).unwrap();

        let mut iter = store.iter();
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
        assert!(store.iter_from(at("10:00:00")).next().is_none());
    }

    #[test]
    fn test_iter_is_not_fused_at_end_of_stream() {
        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();
        let a = count_fact(1, at("10:00:00"));
        let b = count_fact(2, at("10:01:00"));
        store.append(a.clone()).unwrap();

        let mut iter = store.iter();
        assert_eq!(iter.next().unwrap().unwrap(), a);
        assert!(iter.next().is_none());

        store.append(b.clone()).unwrap();
        assert_eq!(iter.next().unwrap().unwrap(), b);
        assert!(iter.next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_iter_stops_after_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("facts.stream");
        let store: FactStore<String, TestValue, String> = FactStore::open_or_create(&path).unwrap();
        // A directory opens fine on Unix, but reading it fails.
        std::fs::create_dir(&path).unwrap();

        let mut iter = store.iter();
        assert!(matches!(iter.next(), Some(Err(ReadError::Io(_)))));
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_iter_yields_open_error_first() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("sub");
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(parent.join("facts.stream")).unwrap();
        // Replace the parent directory with a file: opening the stream now
        // fails with an error other than "not found".
        std::fs::remove_dir(&parent).unwrap();
        std::fs::write(&parent, b"").unwrap();

        let mut iter = store.iter_from(at("10:00:00"));
        assert!(matches!(iter.next(), Some(Err(ReadError::Io(_)))));
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_read_latest_timestamp_skips_malformed_lines() {
        let temp = NamedTempFile::new().unwrap();
        let a = count_fact(1, at("10:00:00"));
        {
            let store: FactStore<String, TestValue, String> =
                FactStore::open_or_create(temp.path()).unwrap();
            store.append(a.clone()).unwrap();
        }
        append_garbage_line(temp.path());

        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();
        assert_eq!(store.latest_timestamp(), Some(*a.timestamp()));
    }

    fn count_fact(n: u32, timestamp: DateTime<Utc>) -> Fact<String, TestValue, String> {
        Fact::new(
            format!("item{n}"),
            TestValue::Count(n),
            timestamp,
            "source1".to_string(),
            Operation::Assert,
        )
    }

    #[test]
    fn test_append_succeeds_while_a_reader_is_mid_iteration() {
        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();
        let facts = create_test_facts();
        store.append_batch(&facts).unwrap();

        let mut reader = store.read_at(StreamPosition::START).unwrap();
        assert_eq!(reader.next().unwrap().unwrap(), facts[0]);
        let mid = reader.position();

        // The reader holds no lock, so the writer is not blocked.
        let later = count_fact(4, "2024-01-15T10:03:00Z".parse().unwrap());
        let end = store.append(later.clone()).unwrap();

        // The same reader tails the stream and sees the new fact.
        let rest: Vec<_> = reader.by_ref().collect::<Result<_, _>>().unwrap();
        assert_eq!(
            rest,
            vec![facts[1].clone(), facts[2].clone(), later.clone()]
        );
        assert_eq!(reader.position(), end);

        // A reader reopened at the earlier position sees it too.
        let reopened: Vec<_> = store
            .read_at(mid)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(reopened, rest);
    }

    #[test]
    fn test_with_lock_timeout_is_applied() {
        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();
        assert_eq!(store.lock_timeout(), DEFAULT_LOCK_TIMEOUT);
        assert_eq!(DEFAULT_LOCK_TIMEOUT, std::time::Duration::from_secs(5));

        let store = store.with_lock_timeout(std::time::Duration::ZERO);
        assert_eq!(store.lock_timeout(), std::time::Duration::ZERO);

        // With a zero timeout, an external exclusive lock fails the append
        // at once.
        use fs2::FileExt;
        let holder = std::fs::OpenOptions::new()
            .write(true)
            .open(temp.path())
            .unwrap();
        holder.try_lock_exclusive().unwrap();
        let result = store.append(create_test_facts()[0].clone());
        assert!(
            matches!(result, Err(StoreError::Write(WriteError::AlreadyLocked))),
            "expected AlreadyLocked, got {result:?}"
        );
        FileExt::unlock(&holder).unwrap();
    }

    #[test]
    fn test_concurrent_appends_all_land_in_timestamp_order() {
        const THREADS: u32 = 8;
        const PER_THREAD: u32 = 25;

        let temp = NamedTempFile::new().unwrap();
        let store: FactStore<String, TestValue, String> =
            FactStore::open_or_create(temp.path()).unwrap();

        std::thread::scope(|scope| {
            for t in 0..THREADS {
                let store = &store;
                scope.spawn(move || {
                    for i in 0..PER_THREAD {
                        let n = t * PER_THREAD + i;
                        // A timestamp taken before the append can lose the
                        // race to another thread's later one; that append is
                        // rightly rejected, so retry with a fresh timestamp.
                        // Any other error (such as a lock failure) fails the
                        // test.
                        loop {
                            match store.append(count_fact(n, Utc::now())) {
                                Ok(_) => break,
                                Err(StoreError::TimestampOrdering { .. }) => continue,
                                Err(e) => panic!("append {n} failed: {e}"),
                            }
                        }
                    }
                });
            }
        });

        let written: Vec<_> = store
            .read_at(StreamPosition::START)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(written.len(), (THREADS * PER_THREAD) as usize);
        assert!(
            written
                .windows(2)
                .all(|w| w[0].timestamp() <= w[1].timestamp()),
            "facts are not in non-decreasing timestamp order"
        );
        let mut values: Vec<u32> = written
            .iter()
            .map(|f| match f.value() {
                TestValue::Count(n) => *n,
            })
            .collect();
        values.sort_unstable();
        assert_eq!(values, (0..THREADS * PER_THREAD).collect::<Vec<_>>());
        assert_eq!(
            store.latest_timestamp(),
            Some(*written.last().unwrap().timestamp())
        );
    }
}
