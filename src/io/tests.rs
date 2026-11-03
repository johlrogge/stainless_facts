use super::*;
use crate::{Fact, Operation};
use fs2::FileExt;
use tempfile::NamedTempFile;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t", content = "v")]
enum TestValue {
    Bpm(u16),
    Title(String),
    Description(String),
}

fn fact(entity: &str, value: TestValue, ts: &str, source: &str) -> Fact<String, TestValue, String> {
    Fact::new(
        entity.to_string(),
        value,
        ts.parse().unwrap(),
        source.to_string(),
        Operation::Assert,
    )
}

#[test]
fn sync_round_trip_single_fact() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let original = fact(
        "track1",
        TestValue::Bpm(12800),
        "2024-01-15T10:00:00Z",
        "alice",
    );

    {
        let mut writer = FactStreamWriter::open(path).unwrap();
        writer.write_batch(std::slice::from_ref(&original)).unwrap();
    }

    let reader = FactStreamReader::open(path).unwrap();
    let facts: Vec<Fact<String, TestValue, String>> = reader.collect::<Result<_, _>>().unwrap();

    assert_eq!(facts, vec![original]);
}

#[test]
fn sync_round_trip_multiple_facts() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let facts = vec![
        fact(
            "track1",
            TestValue::Bpm(12800),
            "2024-01-15T10:00:00Z",
            "alice",
        ),
        fact(
            "track1",
            TestValue::Title("Cool Track".to_string()),
            "2024-01-15T10:01:00Z",
            "alice",
        ),
        fact(
            "track2",
            TestValue::Bpm(13000),
            "2024-01-15T10:02:00Z",
            "bob",
        ),
    ];

    {
        let mut writer = FactStreamWriter::open(path).unwrap();
        writer.write_batch(&facts).unwrap();
    }

    let reader = FactStreamReader::open(path).unwrap();
    let read_facts: Vec<Fact<String, TestValue, String>> =
        reader.collect::<Result<_, _>>().unwrap();

    assert_eq!(read_facts, facts);
}

#[test]
fn sync_round_trip_with_newlines_in_values() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let original = fact(
        "track1",
        TestValue::Description("Line 1\nLine 2\nLine 3".to_string()),
        "2024-01-15T10:00:00Z",
        "alice",
    );

    {
        let mut writer = FactStreamWriter::open(path).unwrap();
        writer.write_batch(std::slice::from_ref(&original)).unwrap();
    }

    let reader = FactStreamReader::open(path).unwrap();
    let facts: Vec<Fact<String, TestValue, String>> = reader.collect::<Result<_, _>>().unwrap();

    assert_eq!(facts, vec![original]);
    match facts[0].value() {
        TestValue::Description(desc) => assert_eq!(desc, "Line 1\nLine 2\nLine 3"),
        _ => panic!("Expected Description variant"),
    }
}

#[test]
fn sync_multiple_batches() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let batch1 = vec![fact(
        "track1",
        TestValue::Bpm(12800),
        "2024-01-15T10:00:00Z",
        "alice",
    )];
    let batch2 = vec![fact(
        "track2",
        TestValue::Bpm(13000),
        "2024-01-15T10:01:00Z",
        "bob",
    )];

    {
        let mut writer = FactStreamWriter::open(path).unwrap();
        writer.write_batch(&batch1).unwrap();
        writer.write_batch(&batch2).unwrap();
    }

    let reader = FactStreamReader::open(path).unwrap();
    let facts: Vec<Fact<String, TestValue, String>> = reader.collect::<Result<_, _>>().unwrap();

    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0], batch1[0]);
    assert_eq!(facts[1], batch2[0]);
}

#[test]
fn sync_write_fails_when_file_is_exclusively_locked() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let mut writer = FactStreamWriter::open(path).unwrap();

    let holder = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    holder.try_lock_exclusive().unwrap();

    let batch = vec![fact(
        "track1",
        TestValue::Bpm(12800),
        "2024-01-15T10:00:00Z",
        "alice",
    )];
    let result = writer.write_batch(&batch);
    assert!(
        matches!(result, Err(WriteError::AlreadyLocked)),
        "expected AlreadyLocked, got {result:?}"
    );

    FileExt::unlock(&holder).unwrap();
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_round_trip_single_fact() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let original = fact(
        "track1",
        TestValue::Bpm(12800),
        "2024-01-15T10:00:00Z",
        "alice",
    );

    {
        let mut writer = AsyncFactStreamWriter::open(path).await.unwrap();
        writer
            .write_batch(std::slice::from_ref(&original))
            .await
            .unwrap();
    }

    let mut reader = AsyncFactStreamReader::open(path).await.unwrap();
    let mut facts: Vec<Fact<String, TestValue, String>> = Vec::new();
    while let Some(result) = reader.next().await {
        facts.push(result.unwrap());
    }

    assert_eq!(facts, vec![original]);
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_round_trip_with_newlines() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let original = fact(
        "track1",
        TestValue::Description("Line 1\nLine 2\nLine 3".to_string()),
        "2024-01-15T10:00:00Z",
        "alice",
    );

    {
        let mut writer = AsyncFactStreamWriter::open(path).await.unwrap();
        writer
            .write_batch(std::slice::from_ref(&original))
            .await
            .unwrap();
    }

    let mut reader = AsyncFactStreamReader::open(path).await.unwrap();
    let mut facts: Vec<Fact<String, TestValue, String>> = Vec::new();
    while let Some(result) = reader.next().await {
        facts.push(result.unwrap());
    }

    assert_eq!(facts, vec![original]);
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_write_fails_when_file_is_exclusively_locked() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let mut writer = AsyncFactStreamWriter::open(path).await.unwrap();

    let holder = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    holder.try_lock_exclusive().unwrap();

    let batch = vec![fact(
        "track1",
        TestValue::Bpm(12800),
        "2024-01-15T10:00:00Z",
        "alice",
    )];
    let result = writer.write_batch(&batch).await;
    assert!(
        matches!(result, Err(WriteError::AlreadyLocked)),
        "expected AlreadyLocked, got {result:?}"
    );

    FileExt::unlock(&holder).unwrap();
}
