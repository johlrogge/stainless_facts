// stainless-facts: AsyncFactStore
// =================================
//
// Add to: src/async_store.rs (new file)

use crate::io::{AsyncFactStreamReader, AsyncFactStreamWriter, ReadError};
use crate::store::{SinceFilter, StoreError, DEFAULT_LOCK_TIMEOUT};
use crate::{Fact, StreamPosition};
use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

/// An async, thread-safe fact store that maintains timestamp ordering.
///
/// The AsyncFactStore provides:
/// - Thread-safe appending of facts with timestamp validation
/// - Resumable reads from a [`StreamPosition`] with [`read_at`](Self::read_at)
/// - Async iteration from any timestamp
/// - Async file I/O with tokio
///
/// # Concurrency
///
/// The same as [`FactStore`](crate::FactStore): appends through one store
/// are serialized by a `tokio::sync::Mutex` held across the timestamp
/// check, the write and the cache update, so concurrent tasks cannot pass
/// the check and then write out of order. Each write also takes the
/// stream's exclusive file lock, waiting up to the store's lock timeout
/// ([`DEFAULT_LOCK_TIMEOUT`](crate::store::DEFAULT_LOCK_TIMEOUT) unless set
/// with [`with_lock_timeout`](Self::with_lock_timeout)) for writers in
/// other processes. Readers take no lock.
///
/// # Examples
///
/// ```rust
/// use stainless_facts::AsyncFactStore;
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
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let store: AsyncFactStore<String, MyValue, String> =
///     AsyncFactStore::open_or_create("facts.stream").await?;
///
/// // Append a fact
/// let fact = Fact::new(
///     "track1".to_string(),
///     MyValue::Bpm(128),
///     Utc::now(),
///     "analyzer".to_string(),
///     Operation::Assert,
/// );
/// store.append(fact).await?;
///
/// // Iterate from a timestamp
/// let since = "2024-01-15T00:00:00Z".parse()?;
/// let mut iter = store.iter_from(since).await;
/// while let Some(fact) = iter.next().await {
///     let fact = fact?;
///     println!("{:?}", fact);
/// }
/// # Ok(())
/// # }
/// ```
pub struct AsyncFactStore<E, V, S> {
    path: PathBuf,
    /// Latest timestamp, cached for quick access. The guard is held for the
    /// whole append, across `.await`, so it also serializes appends.
    latest_timestamp: tokio::sync::Mutex<Option<DateTime<Utc>>>,
    lock_timeout: Duration,
    _phantom: std::marker::PhantomData<(E, V, S)>,
}

impl<E, V, S> AsyncFactStore<E, V, S>
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
    pub async fn open_or_create(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Read latest timestamp if file exists
        let latest_timestamp = if tokio::fs::try_exists(&path).await? {
            Self::read_latest_timestamp(&path).await?
        } else {
            None
        };

        Ok(Self {
            path,
            latest_timestamp: tokio::sync::Mutex::new(latest_timestamp),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
            _phantom: std::marker::PhantomData,
        })
    }

    /// Set how long an append waits for another writer's file lock before
    /// failing with [`WriteError::LockTimeout`](crate::WriteError::LockTimeout).
    ///
    /// The default is [`DEFAULT_LOCK_TIMEOUT`](crate::store::DEFAULT_LOCK_TIMEOUT).
    /// With [`Duration::ZERO`], an append fails at once with
    /// [`WriteError::AlreadyLocked`](crate::WriteError::AlreadyLocked) if the
    /// lock is held.
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
    pub async fn append(&self, fact: Fact<E, V, S>) -> Result<StreamPosition, StoreError> {
        self.append_batch(&[fact]).await
    }

    /// Append a batch of facts, enforcing timestamp ordering.
    ///
    /// If any fact violates timestamp ordering, the entire batch is
    /// rejected. Otherwise the facts are appended contiguously under the
    /// exclusive lock; this is not atomic for concurrent readers (they may
    /// observe a prefix) or across a crash (a torn tail is repaired by the
    /// next writer). See [`AsyncFactStreamWriter::write_batch`].
    ///
    /// Returns the position just past the batch, as returned by
    /// [`AsyncFactStreamWriter::write_batch`]. Pass it to
    /// [`read_at`](Self::read_at) to read only what was appended later. An
    /// empty batch writes no facts and returns the current end of the
    /// stream, taken under the exclusive lock (waiting for it like any
    /// append) after repairing a torn tail, so it is always a valid
    /// position for [`read_at`](Self::read_at). If the store's file does
    /// not exist yet, an empty batch returns [`StreamPosition::START`]
    /// without creating it.
    pub async fn append_batch(
        &self,
        facts: &[Fact<E, V, S>],
    ) -> Result<StreamPosition, StoreError> {
        // Held across the check, the write and the cache update, so that
        // concurrent appends cannot pass the check and then write out of
        // order.
        let mut latest = self.latest_timestamp.lock().await;

        if facts.is_empty() {
            // The end is taken under the file lock, after repairing a torn
            // tail, so it is a line boundary. A missing file is not created.
            return match tokio::fs::metadata(&self.path).await {
                Ok(_) => {
                    let mut writer =
                        AsyncFactStreamWriter::open_with_timeout(&self.path, self.lock_timeout)
                            .await?;
                    Ok(writer.end_position().await?)
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
        let mut writer =
            AsyncFactStreamWriter::open_with_timeout(&self.path, self.lock_timeout).await?;
        let position = writer.write_batch(facts).await?;

        if let Some(last_fact) = facts.last() {
            *latest = Some(*last_fact.timestamp());
        }

        Ok(position)
    }

    /// Read the facts after position `at`.
    ///
    /// The async twin of [`FactStore::read_at`](crate::FactStore::read_at):
    /// `at` is a position returned by [`append`](Self::append),
    /// [`append_batch`](Self::append_batch) or a reader's
    /// [`position`](AsyncFactStreamReader::position), or
    /// [`StreamPosition::START`]. Every fact after it is delivered exactly
    /// once. If the store's file does not exist yet, it is created empty.
    ///
    /// # Errors
    ///
    /// [`StoreError::Read`] with [`ReadError::InvalidPosition`] if `at` is
    /// past the end of the stream or not at the start of a line.
    pub async fn read_at(
        &self,
        at: StreamPosition,
    ) -> Result<AsyncFactStreamReader<E, V, S>, StoreError> {
        // Make sure the file exists, without touching its contents.
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        Ok(AsyncFactStreamReader::open_at(&self.path, at).await?)
    }

    /// Get the latest timestamp in the store.
    ///
    /// Waits for an append in progress through this store to finish. That
    /// append may itself be waiting for another writer's file lock, so this
    /// can take up to the store's [`lock_timeout`](Self::lock_timeout).
    pub async fn latest_timestamp(&self) -> Option<DateTime<Utc>> {
        *self.latest_timestamp.lock().await
    }

    /// Iterate over all facts in the store.
    ///
    /// Each item is a `Result`: a line that fails to decode is yielded as
    /// an `Err` and iteration continues past it. See [`AsyncFactIterator`]
    /// for the error and end-of-stream behaviour.
    pub async fn iter(&self) -> AsyncFactIterator<E, V, S> {
        self.iter_from(DateTime::<Utc>::MIN_UTC).await
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
    pub async fn iter_from(&self, since: DateTime<Utc>) -> AsyncFactIterator<E, V, S> {
        AsyncFactIterator::new(&self.path, since).await
    }

    /// Read the latest timestamp from the file without caching.
    ///
    /// Unlike [`iter`](Self::iter), this skips lines that fail to decode
    /// rather than reporting them, so a malformed line does not prevent
    /// opening the store. I/O errors still fail.
    async fn read_latest_timestamp(path: &Path) -> Result<Option<DateTime<Utc>>, StoreError> {
        let file = tokio::fs::File::open(path).await?;
        let mut reader = BufReader::new(file);

        let mut last_timestamp = None;
        let mut line = String::new();

        // Read through file, keeping track of last timestamp
        // This is O(n) but only done once at startup
        while reader.read_line(&mut line).await? > 0 {
            if let Ok(fact) = Fact::<E, V, S>::from_line(&line) {
                last_timestamp = Some(*fact.timestamp());
            }
            line.clear();
        }

        Ok(last_timestamp)
    }
}

/// Async iterator over the facts in an [`AsyncFactStore`], returned by
/// [`AsyncFactStore::iter`] and [`AsyncFactStore::iter_from`].
///
/// The async twin of [`FactIterator`](crate::FactIterator), built on an
/// [`AsyncFactStreamReader`], with the same behaviour:
///
/// - A line that fails to decode is yielded as
///   `Err(`[`ReadError::Deserialization`]`)` and skipped, even before the
///   starting timestamp is reached.
/// - After an I/O error (`Err(`[`ReadError::Io`]`)`), [`next`](Self::next)
///   returns `None` from then on.
/// - If the store's file does not exist, the iterator is empty.
/// - Any other error opening the file is yielded as the first item.
///
/// Like the reader, it is **not fused**: after `next` returns `None` at the
/// end of the stream, it returns `Some` again once more facts are appended.
/// To read a snapshot, stop at the first `None`. This does not apply after
/// an I/O error or when the file was missing: then it stays empty.
pub struct AsyncFactIterator<E, V, S> {
    /// `None` once iteration has ended for good (missing file, or after an
    /// I/O error).
    reader: Option<AsyncFactStreamReader<E, V, S>>,
    /// An error opening the file, yielded as the first item.
    open_error: Option<ReadError>,
    filter: SinceFilter,
}

impl<E, V, S> AsyncFactIterator<E, V, S> {
    async fn new(path: &Path, since: DateTime<Utc>) -> Self {
        let (reader, open_error) = match AsyncFactStreamReader::open(path).await {
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

    /// Read the next fact, or `None` if no complete line is available yet
    /// (or iteration has ended after an I/O error).
    pub async fn next(&mut self) -> Option<Result<Fact<E, V, S>, ReadError>>
    where
        E: DeserializeOwned,
        V: DeserializeOwned,
        S: DeserializeOwned,
    {
        if let Some(e) = self.open_error.take() {
            return Some(Err(e));
        }
        loop {
            match self.reader.as_mut()?.next().await? {
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
    use crate::{assert_fact_value_format, Fact, Operation, WriteError};
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

    #[tokio::test]
    async fn test_open_or_create() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();

        assert_eq!(store.latest_timestamp().await, None);
    }

    #[tokio::test]
    async fn test_append_and_latest_timestamp() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();

        let facts = create_test_facts();

        store.append(facts[0].clone()).await.unwrap();
        assert_eq!(store.latest_timestamp().await, Some(*facts[0].timestamp()));

        store.append(facts[1].clone()).await.unwrap();
        assert_eq!(store.latest_timestamp().await, Some(*facts[1].timestamp()));
    }

    #[tokio::test]
    async fn test_timestamp_ordering_enforced() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();

        let facts = create_test_facts();

        // Write fact with later timestamp first
        store.append(facts[2].clone()).await.unwrap();

        // Try to write fact with earlier timestamp - should fail
        let result = store.append(facts[0].clone()).await;
        assert!(matches!(result, Err(StoreError::TimestampOrdering { .. })));
    }

    #[tokio::test]
    async fn test_iter_all() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();

        let facts = create_test_facts();
        store.append_batch(&facts).await.unwrap();

        let read_facts = collect_ok(store.iter().await).await;

        assert_eq!(read_facts.len(), 3);
        assert_eq!(read_facts, facts);
    }

    #[tokio::test]
    async fn test_iter_from_middle() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();

        let facts = create_test_facts();
        store.append_batch(&facts).await.unwrap();

        // Start from second fact's timestamp
        let since = *facts[1].timestamp();
        let read_facts = collect_ok(store.iter_from(since).await).await;

        assert_eq!(read_facts.len(), 2);
        assert_eq!(read_facts[0], facts[1]);
        assert_eq!(read_facts[1], facts[2]);
    }

    #[tokio::test]
    async fn test_iter_from_future() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();

        let facts = create_test_facts();
        store.append_batch(&facts).await.unwrap();

        // Start from timestamp after all facts
        let since = "2024-01-15T11:00:00Z".parse().unwrap();
        let read_facts = collect_ok(store.iter_from(since).await).await;

        assert_eq!(read_facts.len(), 0);
    }

    #[tokio::test]
    async fn test_iter_from_past() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();

        let facts = create_test_facts();
        store.append_batch(&facts).await.unwrap();

        // Start from timestamp before all facts
        let since = "2024-01-15T09:00:00Z".parse().unwrap();
        let read_facts = collect_ok(store.iter_from(since).await).await;

        assert_eq!(read_facts.len(), 3);
        assert_eq!(read_facts, facts);
    }

    #[tokio::test]
    async fn test_reopen_and_append() {
        let temp = NamedTempFile::new().unwrap();
        let facts = create_test_facts();

        // Write first fact
        {
            let store: AsyncFactStore<String, TestValue, String> =
                AsyncFactStore::open_or_create(temp.path()).await.unwrap();
            store.append(facts[0].clone()).await.unwrap();
        }

        // Reopen and append second fact
        {
            let store: AsyncFactStore<String, TestValue, String> =
                AsyncFactStore::open_or_create(temp.path()).await.unwrap();

            // Latest timestamp should be restored
            assert_eq!(store.latest_timestamp().await, Some(*facts[0].timestamp()));

            // Should be able to append next fact
            store.append(facts[1].clone()).await.unwrap();
        }

        // Verify both facts are present
        {
            let store: AsyncFactStore<String, TestValue, String> =
                AsyncFactStore::open_or_create(temp.path()).await.unwrap();
            let read_facts = collect_ok(store.iter().await).await;
            assert_eq!(read_facts.len(), 2);
        }
    }

    async fn collect_reader(
        mut reader: crate::io::AsyncFactStreamReader<String, TestValue, String>,
    ) -> Vec<Fact<String, TestValue, String>> {
        let mut facts = Vec::new();
        while let Some(result) = reader.next().await {
            facts.push(result.unwrap());
        }
        facts
    }

    #[tokio::test]
    async fn test_append_returns_positions_and_read_at_resumes() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        let facts = create_test_facts();

        let after_first = store.append(facts[0].clone()).await.unwrap();
        let after_rest = store.append_batch(&facts[1..]).await.unwrap();
        assert!(after_first < after_rest);
        assert_eq!(
            after_rest,
            StreamPosition::from_offset(std::fs::metadata(temp.path()).unwrap().len())
        );

        let resumed = collect_reader(store.read_at(after_first).await.unwrap()).await;
        assert_eq!(resumed, facts[1..]);

        let all = collect_reader(store.read_at(StreamPosition::START).await.unwrap()).await;
        assert_eq!(all, facts);

        let mut at_end = store.read_at(after_rest).await.unwrap();
        assert!(at_end.next().await.is_none());
        assert_eq!(at_end.position(), after_rest);
    }

    #[tokio::test]
    async fn test_empty_append_batch_returns_current_position() {
        let dir = tempfile::tempdir().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(dir.path().join("facts.stream"))
                .await
                .unwrap();

        assert_eq!(
            store.append_batch(&[]).await.unwrap(),
            StreamPosition::START
        );
        let after = store.append_batch(&create_test_facts()).await.unwrap();
        assert_eq!(store.append_batch(&[]).await.unwrap(), after);
    }

    #[tokio::test]
    async fn test_empty_append_batch_on_torn_tail_returns_a_readable_position() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        let a = count_fact(1, at("10:00:00"));
        store.append(a.clone()).await.unwrap();
        {
            // A crashed writer left half a line behind.
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(temp.path())
                .unwrap();
            file.write_all(br#"{"e":"torn"#).unwrap();
        }

        let end = store.append_batch(&[]).await.unwrap();
        assert_eq!(
            end,
            StreamPosition::from_offset(std::fs::metadata(temp.path()).unwrap().len())
        );
        assert!(store.read_at(end).await.unwrap().next().await.is_none());

        let b = count_fact(2, at("10:01:00"));
        store.append(b.clone()).await.unwrap();
        assert_eq!(
            collect_reader(store.read_at(end).await.unwrap()).await,
            vec![b]
        );
    }

    #[tokio::test]
    async fn test_empty_append_batch_on_missing_file_does_not_create_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("facts.stream");
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(&path).await.unwrap();

        assert_eq!(
            store.append_batch(&[]).await.unwrap(),
            StreamPosition::START
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn test_read_at_on_store_without_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(dir.path().join("facts.stream"))
                .await
                .unwrap();

        let facts = collect_reader(store.read_at(StreamPosition::START).await.unwrap()).await;
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn test_read_at_invalid_position_is_a_read_error() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        store.append_batch(&create_test_facts()).await.unwrap();

        let result = store.read_at(StreamPosition::from_offset(1)).await;
        assert!(matches!(
            result,
            Err(StoreError::Read(ReadError::InvalidPosition(_)))
        ));
    }

    #[tokio::test]
    async fn test_empty_store_iter() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();

        let facts = collect_ok(store.iter().await).await;
        assert_eq!(facts.len(), 0);
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

    type TestFact = Fact<String, TestValue, String>;

    async fn collect_ok(mut iter: AsyncFactIterator<String, TestValue, String>) -> Vec<TestFact> {
        let mut facts = Vec::new();
        while let Some(item) = iter.next().await {
            facts.push(item.unwrap());
        }
        facts
    }

    /// Each item as `Some(fact)`, or `None` for a line that failed to
    /// decode. Any other error fails the test.
    async fn outcomes(
        mut iter: AsyncFactIterator<String, TestValue, String>,
    ) -> Vec<Option<TestFact>> {
        let mut items = Vec::new();
        while let Some(item) = iter.next().await {
            items.push(match item {
                Ok(fact) => Some(fact),
                Err(ReadError::Deserialization(_)) => None,
                Err(e) => panic!("expected a decode error, got {e:?}"),
            });
        }
        items
    }

    fn append_garbage_line(path: &Path) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(b"this is not a fact\n").unwrap();
    }

    fn at(time: &str) -> DateTime<Utc> {
        format!("2024-01-15T{time}Z").parse().unwrap()
    }

    #[tokio::test]
    async fn test_iter_yields_err_for_malformed_line_and_continues() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        let a = count_fact(1, at("10:00:00"));
        let b = count_fact(2, at("10:01:00"));
        let c = count_fact(3, at("10:02:00"));

        store.append(a.clone()).await.unwrap();
        append_garbage_line(temp.path());
        store.append_batch(&[b.clone(), c.clone()]).await.unwrap();

        assert_eq!(
            outcomes(store.iter().await).await,
            vec![Some(a), None, Some(b), Some(c)]
        );
    }

    #[tokio::test]
    async fn test_iter_from_yields_err_for_malformed_line_and_continues() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        let early = count_fact(0, at("09:00:00"));
        let a = count_fact(1, at("10:00:00"));
        let b = count_fact(2, at("10:01:00"));
        let c = count_fact(3, at("10:02:00"));

        store.append_batch(&[early, a.clone()]).await.unwrap();
        append_garbage_line(temp.path());
        store.append_batch(&[b.clone(), c.clone()]).await.unwrap();

        assert_eq!(
            outcomes(store.iter_from(*a.timestamp()).await).await,
            vec![Some(a), None, Some(b.clone()), Some(c.clone())]
        );
        assert_eq!(
            outcomes(store.iter_from(*b.timestamp()).await).await,
            vec![None, Some(b), Some(c)]
        );
    }

    #[tokio::test]
    async fn test_iter_on_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(dir.path().join("facts.stream"))
                .await
                .unwrap();

        let mut iter = store.iter().await;
        assert!(iter.next().await.is_none());
        assert!(iter.next().await.is_none());
        assert!(store.iter_from(at("10:00:00")).await.next().await.is_none());
    }

    #[tokio::test]
    async fn test_iter_is_not_fused_at_end_of_stream() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        let a = count_fact(1, at("10:00:00"));
        let b = count_fact(2, at("10:01:00"));
        store.append(a.clone()).await.unwrap();

        let mut iter = store.iter().await;
        assert_eq!(iter.next().await.unwrap().unwrap(), a);
        assert!(iter.next().await.is_none());

        store.append(b.clone()).await.unwrap();
        assert_eq!(iter.next().await.unwrap().unwrap(), b);
        assert!(iter.next().await.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_iter_stops_after_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("facts.stream");
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(&path).await.unwrap();
        // A directory opens fine on Unix, but reading it fails.
        std::fs::create_dir(&path).unwrap();

        let mut iter = store.iter().await;
        assert!(matches!(iter.next().await, Some(Err(ReadError::Io(_)))));
        assert!(iter.next().await.is_none());
        assert!(iter.next().await.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_iter_yields_open_error_first() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("sub");
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(parent.join("facts.stream"))
                .await
                .unwrap();
        std::fs::remove_dir(&parent).unwrap();
        std::fs::write(&parent, b"").unwrap();

        let mut iter = store.iter_from(at("10:00:00")).await;
        assert!(matches!(iter.next().await, Some(Err(ReadError::Io(_)))));
        assert!(iter.next().await.is_none());
    }

    #[tokio::test]
    async fn test_read_latest_timestamp_skips_malformed_lines() {
        let temp = NamedTempFile::new().unwrap();
        let a = count_fact(1, at("10:00:00"));
        {
            let store: AsyncFactStore<String, TestValue, String> =
                AsyncFactStore::open_or_create(temp.path()).await.unwrap();
            store.append(a.clone()).await.unwrap();
        }
        append_garbage_line(temp.path());

        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        assert_eq!(store.latest_timestamp().await, Some(*a.timestamp()));
    }

    #[tokio::test]
    async fn test_store_error_is_the_crate_root_store_error() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        store.append(count_fact(2, at("10:01:00"))).await.unwrap();

        // Compiles only if the async store's error is `crate::StoreError`.
        let result: Result<StreamPosition, crate::StoreError> =
            store.append(count_fact(1, at("10:00:00"))).await;
        assert!(matches!(
            result,
            Err(crate::StoreError::TimestampOrdering { .. })
        ));
    }

    #[tokio::test]
    async fn test_append_succeeds_while_a_reader_is_mid_iteration() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        let facts = create_test_facts();
        store.append_batch(&facts).await.unwrap();

        let mut reader = store.read_at(StreamPosition::START).await.unwrap();
        assert_eq!(reader.next().await.unwrap().unwrap(), facts[0]);
        let mid = reader.position();

        // The reader holds no lock, so the writer is not blocked.
        let later = count_fact(4, "2024-01-15T10:03:00Z".parse().unwrap());
        let end = store.append(later.clone()).await.unwrap();

        // The same reader tails the stream and sees the new fact.
        let mut rest = Vec::new();
        while let Some(result) = reader.next().await {
            rest.push(result.unwrap());
        }
        assert_eq!(rest, vec![facts[1].clone(), facts[2].clone(), later]);
        assert_eq!(reader.position(), end);

        // A reader reopened at the earlier position sees it too.
        assert_eq!(
            collect_reader(store.read_at(mid).await.unwrap()).await,
            rest
        );
    }

    #[tokio::test]
    async fn test_with_lock_timeout_is_applied() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();
        assert_eq!(store.lock_timeout(), crate::store::DEFAULT_LOCK_TIMEOUT);

        let store = store.with_lock_timeout(std::time::Duration::ZERO);
        assert_eq!(store.lock_timeout(), std::time::Duration::ZERO);

        use fs2::FileExt;
        let holder = std::fs::OpenOptions::new()
            .write(true)
            .open(temp.path())
            .unwrap();
        holder.try_lock_exclusive().unwrap();
        let result = store.append(create_test_facts()[0].clone()).await;
        assert!(
            matches!(result, Err(StoreError::Write(WriteError::AlreadyLocked))),
            "expected AlreadyLocked, got {result:?}"
        );
        FileExt::unlock(&holder).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_appends_all_land_in_timestamp_order() {
        const TASKS: u32 = 8;
        const PER_TASK: u32 = 25;

        let temp = NamedTempFile::new().unwrap();
        let store: std::sync::Arc<AsyncFactStore<String, TestValue, String>> =
            std::sync::Arc::new(AsyncFactStore::open_or_create(temp.path()).await.unwrap());

        let mut handles = Vec::new();
        for t in 0..TASKS {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                for i in 0..PER_TASK {
                    let n = t * PER_TASK + i;
                    // See the sync test: a timestamp taken before the append
                    // can lose the race and be rightly rejected; retry with a
                    // fresh one. Any other error fails the test.
                    loop {
                        match store.append(count_fact(n, Utc::now())).await {
                            Ok(_) => break,
                            Err(StoreError::TimestampOrdering { .. }) => continue,
                            Err(e) => panic!("append {n} failed: {e}"),
                        }
                    }
                }
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }

        let written = collect_reader(store.read_at(StreamPosition::START).await.unwrap()).await;
        assert_eq!(written.len(), (TASKS * PER_TASK) as usize);
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
        assert_eq!(values, (0..TASKS * PER_TASK).collect::<Vec<_>>());
        assert_eq!(
            store.latest_timestamp().await,
            Some(*written.last().unwrap().timestamp())
        );
    }
}
