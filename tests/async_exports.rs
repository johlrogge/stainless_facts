//! The async store's types are nameable from the crate root, and its errors
//! are the same `stainless_facts::StoreError` as the sync store's.

#![cfg(feature = "tokio")]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use stainless_facts::{
    AsyncFactIterator, AsyncFactStore, AsyncFactStreamReader, AsyncFactStreamWriter, Fact,
    FactStore, Operation, ReadError, StoreError, StreamPosition,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum Value {
    Count(u32),
}

type TestFact = Fact<String, Value, String>;

fn fact(n: u32, timestamp: &str) -> TestFact {
    let timestamp: DateTime<Utc> = timestamp.parse().unwrap();
    Fact::new(
        format!("item{n}"),
        Value::Count(n),
        timestamp,
        "test".to_string(),
        Operation::Assert,
    )
}

/// Accepts errors from either store: they must be the same type.
fn describe(error: &StoreError) -> &'static str {
    match error {
        StoreError::TimestampOrdering { .. } => "ordering",
        _ => "other",
    }
}

async fn open_async(
    path: &std::path::Path,
) -> Result<AsyncFactStore<String, Value, String>, StoreError> {
    AsyncFactStore::open_or_create(path).await
}

#[tokio::test]
async fn async_and_sync_store_errors_are_stainless_facts_store_error() {
    let dir = tempfile::tempdir().unwrap();

    let async_store = open_async(&dir.path().join("async.stream")).await.unwrap();
    async_store
        .append(fact(2, "2024-01-15T10:01:00Z"))
        .await
        .unwrap();
    let async_error: StoreError = async_store
        .append(fact(1, "2024-01-15T10:00:00Z"))
        .await
        .unwrap_err();

    let sync_store: FactStore<String, Value, String> =
        FactStore::open_or_create(dir.path().join("sync.stream")).unwrap();
    sync_store.append(fact(2, "2024-01-15T10:01:00Z")).unwrap();
    let sync_error: StoreError = sync_store
        .append(fact(1, "2024-01-15T10:00:00Z"))
        .unwrap_err();

    assert_eq!(describe(&async_error), "ordering");
    assert_eq!(describe(&sync_error), "ordering");
}

#[tokio::test]
async fn async_stream_types_are_nameable_from_the_crate_root() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("facts.stream");
    let written = fact(1, "2024-01-15T10:00:00Z");

    let mut writer: AsyncFactStreamWriter = AsyncFactStreamWriter::open(&path).await.unwrap();
    let end: StreamPosition = writer
        .write_batch(std::slice::from_ref(&written))
        .await
        .unwrap();

    let mut reader: AsyncFactStreamReader<String, Value, String> =
        AsyncFactStreamReader::open(&path).await.unwrap();
    let read: Option<Result<TestFact, ReadError>> = reader.next().await;
    assert_eq!(read.unwrap().unwrap(), written);
    assert_eq!(reader.position(), end);

    let store = open_async(&path).await.unwrap();
    let mut iter: AsyncFactIterator<String, Value, String> = store.iter().await;
    let item: Option<Result<TestFact, ReadError>> = iter.next().await;
    assert_eq!(item.unwrap().unwrap(), written);
}
