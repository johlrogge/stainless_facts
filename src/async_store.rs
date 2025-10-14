// stainless-facts: AsyncFactStore
// =================================
//
// Add to: src/async_store.rs (new file)

use crate::io::{AsyncFactStreamWriter, ReadError, WriteError};
use crate::Fact;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};
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
/// - Efficient async iteration from any timestamp
/// - Read-write locking for concurrent access
/// - Async file I/O with tokio
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
    /// Latest timestamp, cached for quick access
    latest_timestamp: RwLock<Option<DateTime<Utc>>>,
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
            latest_timestamp: RwLock::new(latest_timestamp),
            _phantom: std::marker::PhantomData,
        })
    }

    /// Append a single fact, enforcing timestamp ordering.
    pub async fn append(&self, fact: Fact<E, V, S>) -> Result<(), StoreError> {
        self.append_batch(&[fact]).await
    }

    /// Append a batch of facts, enforcing timestamp ordering.
    ///
    /// All facts are written atomically. If any fact violates timestamp ordering,
    /// the entire batch is rejected.
    pub async fn append_batch(&self, facts: &[Fact<E, V, S>]) -> Result<(), StoreError> {
        if facts.is_empty() {
            return Ok(());
        }

        // Check timestamp ordering
        let latest = *self.latest_timestamp.read();

        for fact in facts {
            if let Some(latest_ts) = latest {
                if fact.timestamp() < &latest_ts {
                    return Err(StoreError::TimestampOrdering {
                        new: *fact.timestamp(),
                        latest: latest_ts,
                    });
                }
            }
        }

        // Write facts (AsyncFactStreamWriter handles locking)
        let mut writer = AsyncFactStreamWriter::open(&self.path).await?;
        writer.write_batch(facts).await?;

        // Update cached latest timestamp
        if let Some(last_fact) = facts.last() {
            let mut latest = self.latest_timestamp.write();
            *latest = Some(*last_fact.timestamp());
        }

        Ok(())
    }

    /// Get the latest timestamp in the store.
    pub fn latest_timestamp(&self) -> Option<DateTime<Utc>> {
        *self.latest_timestamp.read()
    }

    /// Iterate over all facts in the store.
    pub async fn iter(&self) -> AsyncFactIterator<E, V, S> {
        self.iter_from(DateTime::<Utc>::MIN_UTC).await
    }

    /// Iterate over facts starting from a specific timestamp.
    ///
    /// This efficiently seeks to the first fact at or after the given timestamp.
    ///
    /// # Performance
    ///
    /// Currently performs a linear scan from the start. For large fact streams,
    /// consider adding an index file for faster seeking.
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
            if let Ok(fact) = serde_json::from_str::<Fact<E, V, S>>(&line) {
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
            let fact: Fact<E, V, S> = serde_json::from_str(&self.line_buffer).ok()?;

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

        assert_eq!(store.latest_timestamp(), None);
    }

    #[tokio::test]
    async fn test_append_and_latest_timestamp() {
        let temp = NamedTempFile::new().unwrap();
        let store: AsyncFactStore<String, TestValue, String> =
            AsyncFactStore::open_or_create(temp.path()).await.unwrap();

        let facts = create_test_facts();

        store.append(facts[0].clone()).await.unwrap();
        assert_eq!(store.latest_timestamp(), Some(facts[0].timestamp().clone()));

        store.append(facts[1].clone()).await.unwrap();
        assert_eq!(store.latest_timestamp(), Some(facts[1].timestamp().clone()));
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
        let since = facts[1].timestamp().clone();
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
            assert_eq!(store.latest_timestamp(), Some(facts[0].timestamp().clone()));

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
}
