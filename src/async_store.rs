// stainless-facts: AsyncFactStore
// =================================
//
// Add to: src/async_store.rs (new file)

use crate::io::{AsyncFactStreamReader, AsyncFactStreamWriter, ReadError, WriteError};
use crate::store::DEFAULT_LOCK_TIMEOUT;
use crate::{Fact, StreamPosition};
use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};

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
    /// failing with [`WriteError::LockTimeout`].
    ///
    /// The default is [`DEFAULT_LOCK_TIMEOUT`](crate::store::DEFAULT_LOCK_TIMEOUT).
    /// With [`Duration::ZERO`], an append fails at once with
    /// [`WriteError::AlreadyLocked`] if the lock is held.
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
    /// All facts are written atomically. If any fact violates timestamp ordering,
    /// the entire batch is rejected.
    ///
    /// Returns the position just past the batch, as returned by
    /// [`AsyncFactStreamWriter::write_batch`]. Pass it to
    /// [`read_at`](Self::read_at) to read only what was appended later. An
    /// empty batch writes nothing and returns the current end of the stream.
    pub async fn append_batch(
        &self,
        facts: &[Fact<E, V, S>],
    ) -> Result<StreamPosition, StoreError> {
        if facts.is_empty() {
            return match tokio::fs::metadata(&self.path).await {
                Ok(metadata) => Ok(StreamPosition::from_offset(metadata.len())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StreamPosition::START),
                Err(e) => Err(e.into()),
            };
        }

        // Held across the check, the write and the cache update, so that
        // concurrent appends cannot pass the check and then write out of
        // order.
        let mut latest = self.latest_timestamp.lock().await;

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
    /// `StoreError::Read` with [`ReadError::InvalidPosition`] if `at` is
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
    /// Waits for an append in progress to finish.
    pub async fn latest_timestamp(&self) -> Option<DateTime<Utc>> {
        *self.latest_timestamp.lock().await
    }

    /// Iterate over all facts in the store.
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
    /// # Performance
    ///
    /// Performs a linear scan from the start of the stream.
    pub async fn iter_from(&self, since: DateTime<Utc>) -> AsyncFactIterator<E, V, S> {
        AsyncFactIterator::new(self.path.clone(), since).await
    }

    /// Read the latest timestamp from the file without caching.
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

/// Async iterator over facts in a fact store.
///
/// Lazily reads facts from disk, yielding only those at or after the starting timestamp.
pub struct AsyncFactIterator<E, V, S> {
    reader: BufReader<tokio::fs::File>,
    since: DateTime<Utc>,
    line_buffer: String,
    found_starting_point: bool,
    _phantom: std::marker::PhantomData<(E, V, S)>,
}

impl<E, V, S> AsyncFactIterator<E, V, S>
where
    E: DeserializeOwned + Clone,
    V: DeserializeOwned + Clone,
    S: DeserializeOwned + Clone,
{
    async fn new(path: PathBuf, since: DateTime<Utc>) -> Self {
        let file = tokio::fs::File::open(&path).await.ok();
        let reader = file.map(BufReader::new).unwrap_or_else(|| {
            // Return empty reader if file doesn't exist
            // Use a never-ready file
            BufReader::new(tokio::fs::File::from_std(
                std::fs::File::open("/dev/null").unwrap(),
            ))
        });

        Self {
            reader,
            since,
            line_buffer: String::with_capacity(1024),
            found_starting_point: false,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Read the next fact from the iterator.
    pub async fn next(&mut self) -> Option<Fact<E, V, S>> {
        loop {
            self.line_buffer.clear();

            // Read next line
            let bytes_read = self.reader.read_line(&mut self.line_buffer).await.ok()?;
            if bytes_read == 0 {
                return None; // EOF
            }

            // Parse fact
            let fact: Fact<E, V, S> = Fact::from_line(&self.line_buffer).ok()?;

            // If we haven't found starting point yet, check timestamp
            if !self.found_starting_point {
                if fact.timestamp() >= &self.since {
                    self.found_starting_point = true;
                    return Some(fact);
                }
                // Skip this fact, continue searching
                continue;
            }

            // We're past starting point, return all facts
            return Some(fact);
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

        let mut read_facts = Vec::new();
        let mut iter = store.iter().await;
        while let Some(fact) = iter.next().await {
            read_facts.push(fact);
        }

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
        let mut read_facts = Vec::new();
        let mut iter = store.iter_from(since).await;
        while let Some(fact) = iter.next().await {
            read_facts.push(fact);
        }

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
        let mut read_facts = Vec::new();
        let mut iter = store.iter_from(since).await;
        while let Some(fact) = iter.next().await {
            read_facts.push(fact);
        }

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
        let mut read_facts = Vec::new();
        let mut iter = store.iter_from(since).await;
        while let Some(fact) = iter.next().await {
            read_facts.push(fact);
        }

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
            let mut read_facts = Vec::new();
            let mut iter = store.iter().await;
            while let Some(fact) = iter.next().await {
                read_facts.push(fact);
            }
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

        let mut facts = Vec::new();
        let mut iter = store.iter().await;
        while let Some(fact) = iter.next().await {
            facts.push(fact);
        }
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
