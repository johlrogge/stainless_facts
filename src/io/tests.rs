#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Fact, Operation};
    use tempfile::NamedTempFile;

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(tag = "t", content = "v")]
    enum TestValue {
        Bpm(u16),
        Title(String),
        Description(String),
    }

    #[test]
    fn sync_round_trip_single_fact() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path();

        let original = Fact::new(
            "track1".to_string(),
            TestValue::Bpm(12800),
            "2024-01-15T10:00:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert,
        );

        // Write
        {
            let mut writer = FactStreamWriter::open(path).unwrap();
            writer.write_batch(&[original.clone()]).unwrap();
        }

        // Read
        let reader = FactStreamReader::open(path).unwrap();
        let facts: Result<Vec<_>, _> = reader.collect();
        let facts = facts.unwrap();

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0], original);
    }

    #[test]
    fn sync_round_trip_multiple_facts() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path();

        let facts = vec![
            Fact::new(
                "track1".to_string(),
                TestValue::Bpm(12800),
                "2024-01-15T10:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert,
            ),
            Fact::new(
                "track1".to_string(),
                TestValue::Title("Cool Track".to_string()),
                "2024-01-15T10:01:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert,
            ),
            Fact::new(
                "track2".to_string(),
                TestValue::Bpm(13000),
                "2024-01-15T10:02:00Z".parse().unwrap(),
                "bob".to_string(),
                Operation::Assert,
            ),
        ];

        // Write
        {
            let mut writer = FactStreamWriter::open(path).unwrap();
            writer.write_batch(&facts).unwrap();
        }

        // Read
        let reader = FactStreamReader::open(path).unwrap();
        let read_facts: Result<Vec<_>, _> = reader.collect();
        let read_facts = read_facts.unwrap();

        assert_eq!(read_facts, facts);
    }

    #[test]
    fn sync_round_trip_with_newlines_in_values() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path();

        let original = Fact::new(
            "track1".to_string(),
            TestValue::Description("Line 1\nLine 2\nLine 3".to_string()),
            "2024-01-15T10:00:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert,
        );

        // Write
        {
            let mut writer = FactStreamWriter::open(path).unwrap();
            writer.write_batch(&[original.clone()]).unwrap();
        }

        // Read
        let reader = FactStreamReader::open(path).unwrap();
        let facts: Result<Vec<_>, _> = reader.collect();
        let facts = facts.unwrap();

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0], original);

        // Verify the description preserved newlines
        match facts[0].value() {
            TestValue::Description(desc) => {
                assert_eq!(desc, "Line 1\nLine 2\nLine 3");
            }
            _ => panic!("Expected Description variant"),
        }
    }

    #[test]
    fn sync_multiple_batches() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path();

        let batch1 = vec![Fact::new(
            "track1".to_string(),
            TestValue::Bpm(12800),
            "2024-01-15T10:00:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert,
        )];

        let batch2 = vec![Fact::new(
            "track2".to_string(),
            TestValue::Bpm(13000),
            "2024-01-15T10:01:00Z".parse().unwrap(),
            "bob".to_string(),
            Operation::Assert,
        )];

        // Write two batches
        {
            let mut writer = FactStreamWriter::open(path).unwrap();
            writer.write_batch(&batch1).unwrap();
            writer.write_batch(&batch2).unwrap();
        }

        // Read all
        let reader = FactStreamReader::open(path).unwrap();
        let facts: Result<Vec<_>, _> = reader.collect();
        let facts = facts.unwrap();

        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0], batch1[0]);
        assert_eq!(facts[1], batch2[0]);
    }

    #[test]
    fn sync_writer_lock_prevents_second_writer() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path();

        let _writer1 = FactStreamWriter::open(path).unwrap();

        // Second writer should fail immediately
        let result = FactStreamWriter::open(path);
        assert!(matches!(result, Err(WriteError::AlreadyLocked)));
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn async_round_trip_single_fact() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path();

        let original = Fact::new(
            "track1".to_string(),
            TestValue::Bpm(12800),
            "2024-01-15T10:00:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert,
        );

        // Write
        {
            let mut writer = AsyncFactStreamWriter::open(path).await.unwrap();
            writer.write_batch(&[original.clone()]).await.unwrap();
        }

        // Read
        let mut reader = AsyncFactStreamReader::open(path).await.unwrap();
        let mut facts = Vec::new();

        while let Some(result) = reader.next().await {
            facts.push(result.unwrap());
        }

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0], original);
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn async_round_trip_with_newlines() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path();

        let original = Fact::new(
            "track1".to_string(),
            TestValue::Description("Line 1\nLine 2\nLine 3".to_string()),
            "2024-01-15T10:00:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert,
        );

        // Write
        {
            let mut writer = AsyncFactStreamWriter::open(path).await.unwrap();
            writer.write_batch(&[original.clone()]).await.unwrap();
        }

        // Read
        let mut reader = AsyncFactStreamReader::open(path).await.unwrap();
        let mut facts = Vec::new();

        while let Some(result) = reader.next().await {
            facts.push(result.unwrap());
        }

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0], original);
    }
}
