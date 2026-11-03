use super::*;
use crate::{Fact, Operation, StreamPosition};
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

#[test]
fn writer_output_is_byte_identical_to_to_line_plus_newline() {
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
            "track2",
            TestValue::Description("Line 1\nLine 2\r\nLine 3".to_string()),
            "2024-01-15T10:01:00Z",
            "bob",
        ),
    ];

    {
        let mut writer = FactStreamWriter::open(path).unwrap();
        writer.write_batch(&facts).unwrap();
    }

    let expected: String = facts
        .iter()
        .map(|f| format!("{}\n", f.to_line().unwrap()))
        .collect();
    assert_eq!(std::fs::read(path).unwrap(), expected.into_bytes());

    // And every written line reads back through `from_line`.
    let text = std::fs::read_to_string(path).unwrap();
    let decoded: Vec<Fact<String, TestValue, String>> =
        text.lines().map(|l| Fact::from_line(l).unwrap()).collect();
    assert_eq!(decoded, facts);
}

// ============================================================================
// StreamPosition: positioned reads and writes
// ============================================================================

type TestFact = Fact<String, TestValue, String>;

fn numbered(n: u16) -> TestFact {
    fact(
        &format!("track{n}"),
        TestValue::Bpm(n),
        "2024-01-15T10:00:00Z",
        "alice",
    )
}

fn file_len(path: &std::path::Path) -> StreamPosition {
    StreamPosition::from_offset(std::fs::metadata(path).unwrap().len())
}

fn append_raw(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
}

fn read_all_at(path: &std::path::Path, at: StreamPosition) -> Vec<TestFact> {
    FactStreamReader::open_at(path, at)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Three batches of one, two and three facts, with the position returned
/// after each batch.
fn write_three_batches(path: &std::path::Path) -> (Vec<Vec<TestFact>>, Vec<StreamPosition>) {
    let batches = vec![
        vec![numbered(1)],
        vec![numbered(2), numbered(3)],
        vec![numbered(4), numbered(5), numbered(6)],
    ];
    let mut writer = FactStreamWriter::open(path).unwrap();
    let positions = batches
        .iter()
        .map(|batch| writer.write_batch(batch).unwrap())
        .collect();
    (batches, positions)
}

#[test]
fn sync_reopen_at_each_batch_position_yields_exactly_the_later_facts() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let (batches, positions) = write_three_batches(path);

    assert!(positions[0] < positions[1] && positions[1] < positions[2]);

    for (i, at) in positions.iter().enumerate() {
        let expected: Vec<TestFact> = batches[i + 1..].concat();
        assert_eq!(read_all_at(path, *at), expected, "after batch {i}");
    }
    assert_eq!(read_all_at(path, StreamPosition::START), batches.concat());
}

#[test]
fn sync_position_after_full_iteration_equals_file_length() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    write_three_batches(path);

    let mut reader = FactStreamReader::<String, TestValue, String>::open(path).unwrap();
    assert_eq!(reader.position(), StreamPosition::START);
    for result in reader.by_ref() {
        result.unwrap();
    }
    assert_eq!(reader.position(), file_len(path));
}

#[test]
fn sync_write_batch_position_equals_reader_position_at_eof() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let (_, positions) = write_three_batches(path);

    let mut reader = FactStreamReader::<String, TestValue, String>::open(path).unwrap();
    while reader.next().is_some() {}
    assert_eq!(reader.position(), *positions.last().unwrap());
    assert_eq!(*positions.last().unwrap(), file_len(path));
}

#[test]
fn sync_open_at_start_behaves_like_open() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    write_three_batches(path);

    let mut from_open = FactStreamReader::<String, TestValue, String>::open(path).unwrap();
    let mut from_start =
        FactStreamReader::<String, TestValue, String>::open_at(path, StreamPosition::START)
            .unwrap();
    assert_eq!(from_open.position(), from_start.position());
    loop {
        let a = from_open.next().map(Result::unwrap);
        let b = from_start.next().map(Result::unwrap);
        assert_eq!(a, b);
        assert_eq!(from_open.position(), from_start.position());
        if a.is_none() {
            break;
        }
    }
}

#[test]
fn sync_partial_trailing_line_is_not_yielded_until_terminated() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let first = numbered(1);
    let second = numbered(2);
    let after_first = FactStreamWriter::open(path)
        .unwrap()
        .write_batch(std::slice::from_ref(&first))
        .unwrap();
    let line = second.to_line().unwrap();
    let (head, tail) = line.as_bytes().split_at(line.len() / 2);
    append_raw(path, head);

    let mut reader = FactStreamReader::<String, TestValue, String>::open(path).unwrap();
    assert_eq!(reader.next().unwrap().unwrap(), first);
    assert!(reader.next().is_none());
    assert_eq!(reader.position(), after_first);

    // The writer finishes the line but not the newline: still not yielded.
    append_raw(path, tail);
    assert!(reader.next().is_none());
    assert_eq!(reader.position(), after_first);

    // Once the newline lands, the same reader yields it (tailing).
    append_raw(path, b"\n");
    assert_eq!(reader.next().unwrap().unwrap(), second);
    assert_eq!(reader.position(), file_len(path));
    assert!(reader.next().is_none());
}

#[test]
fn sync_empty_lines_are_skipped_without_corrupting_the_position() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let first = numbered(1);
    let second = numbered(2);
    let first_line = format!("{}\n", first.to_line().unwrap());
    append_raw(path, first_line.as_bytes());
    append_raw(path, b"\n   \n\t\r\n");
    append_raw(
        path,
        format!("{}\n\n", second.to_line().unwrap()).as_bytes(),
    );

    let mut reader = FactStreamReader::<String, TestValue, String>::open(path).unwrap();
    assert_eq!(reader.next().unwrap().unwrap(), first);
    assert_eq!(
        reader.position(),
        StreamPosition::from_offset(first_line.len() as u64)
    );
    assert_eq!(reader.next().unwrap().unwrap(), second);
    assert!(reader.next().is_none());
    assert_eq!(reader.position(), file_len(path));

    // The position after the first fact is still a valid resume point.
    let resumed = read_all_at(path, StreamPosition::from_offset(first_line.len() as u64));
    assert_eq!(resumed, vec![second]);
}

#[test]
fn sync_decode_error_is_yielded_and_advanced_past() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let first = numbered(1);
    let second = numbered(2);
    let good = format!("{}\n", first.to_line().unwrap());
    append_raw(path, good.as_bytes());
    append_raw(path, b"this is not a fact\n");
    append_raw(path, format!("{}\n", second.to_line().unwrap()).as_bytes());

    let mut reader = FactStreamReader::<String, TestValue, String>::open(path).unwrap();
    assert_eq!(reader.next().unwrap().unwrap(), first);
    assert!(matches!(
        reader.next(),
        Some(Err(ReadError::Deserialization(_)))
    ));
    assert_eq!(
        reader.position(),
        StreamPosition::from_offset((good.len() + "this is not a fact\n".len()) as u64)
    );
    assert_eq!(reader.next().unwrap().unwrap(), second);
    assert!(reader.next().is_none());
    assert_eq!(reader.position(), file_len(path));
}

#[test]
fn sync_open_at_mid_line_or_past_eof_is_invalid_position() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let (_, positions) = write_three_batches(path);

    let mid_line = StreamPosition::from_offset(positions[0].offset() + 3);
    let past_eof = StreamPosition::from_offset(positions[2].offset() + 1);
    for bad in [mid_line, past_eof, StreamPosition::from_offset(1)] {
        let result = FactStreamReader::<String, TestValue, String>::open_at(path, bad);
        assert!(
            matches!(result, Err(ReadError::InvalidPosition(p)) if p == bad),
            "expected InvalidPosition({bad:?})"
        );
    }

    // Exactly at EOF is valid and yields nothing.
    assert!(read_all_at(path, positions[2]).is_empty());
}

#[test]
fn sync_open_at_start_of_empty_file_is_valid() {
    let temp = NamedTempFile::new().unwrap();
    assert!(read_all_at(temp.path(), StreamPosition::START).is_empty());
}

#[test]
fn sync_read_lines_at_pages_through_complete_lines() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let lines: Vec<String> = (1..=5).map(|n| numbered(n).to_line().unwrap()).collect();
    append_raw(path, format!("{}\n\n{}\n", lines[0], lines[1]).as_bytes());
    append_raw(path, format!("  \n{}\n{}\n", lines[2], lines[3]).as_bytes());
    // A partial tail: the writer hasn't finished this line yet.
    append_raw(path, lines[4].as_bytes());

    let (page1, at1) = read_lines_at(path, StreamPosition::START, 2).unwrap();
    assert_eq!(page1, lines[0..2]);

    let (page2, at2) = read_lines_at(path, at1, 2).unwrap();
    assert_eq!(page2, lines[2..4]);

    let (page3, at3) = read_lines_at(path, at2, 2).unwrap();
    assert!(page3.is_empty());
    assert_eq!(at3, at2);
    assert_eq!(
        at2,
        StreamPosition::from_offset(file_len(path).offset() - lines[4].len() as u64)
    );

    append_raw(path, b"\n");
    let (page4, at4) = read_lines_at(path, at3, 10).unwrap();
    assert_eq!(page4, lines[4..5]);
    assert_eq!(at4, file_len(path));

    // A zero limit reads nothing and stays put.
    assert_eq!(
        read_lines_at(path, at1, 0).unwrap(),
        (Vec::<String>::new(), at1)
    );

    // Lines are returned verbatim, so they decode with the public codec.
    let decoded: TestFact = Fact::from_line(&page1[0]).unwrap();
    assert_eq!(decoded, numbered(1));
}

#[test]
fn sync_read_lines_at_strips_crlf() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let lines: Vec<String> = (1..=2).map(|n| numbered(n).to_line().unwrap()).collect();
    append_raw(path, format!("{}\r\n{}\n", lines[0], lines[1]).as_bytes());

    let (page, at) = read_lines_at(path, StreamPosition::START, 10).unwrap();
    assert_eq!(page, lines);
    assert_eq!(at, file_len(path));
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_read_lines_at_strips_crlf() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let lines: Vec<String> = (1..=2).map(|n| numbered(n).to_line().unwrap()).collect();
    append_raw(path, format!("{}\r\n{}\n", lines[0], lines[1]).as_bytes());

    let (page, at) = read_lines_at_async(path, StreamPosition::START, 10)
        .await
        .unwrap();
    assert_eq!(page, lines);
    assert_eq!(at, file_len(path));
}

#[test]
fn sync_read_lines_at_invalid_utf8_errors_without_advancing() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let good = numbered(1).to_line().unwrap();
    append_raw(path, format!("{good}\n").as_bytes());
    append_raw(path, b"\xff\n");

    let (page, at) = read_lines_at(path, StreamPosition::START, 1).unwrap();
    assert_eq!(page, vec![good]);

    // The bad line fails the call, and the caller's position is unchanged,
    // so retrying at the same position fails again.
    for _ in 0..2 {
        let result = read_lines_at(path, at, 10);
        assert!(
            matches!(&result, Err(ReadError::Io(e)) if e.kind() == std::io::ErrorKind::InvalidData),
            "expected InvalidData, got {result:?}"
        );
    }
    // From the start, the whole call fails too.
    assert!(read_lines_at(path, StreamPosition::START, 10).is_err());
}

#[test]
fn sync_read_lines_at_rejects_invalid_position() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    write_three_batches(path);

    let result = read_lines_at(path, StreamPosition::from_offset(2), 10);
    assert!(matches!(result, Err(ReadError::InvalidPosition(_))));
}

// ============================================================================
// Write path: lock timeout, torn tails
// ============================================================================

/// An external handle holding the exclusive lock on `path`.
fn external_lock(path: &std::path::Path) -> std::fs::File {
    let holder = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    holder.try_lock_exclusive().unwrap();
    holder
}

/// Each item from `path` as `Some(fact)`, or `None` for a line that failed
/// to decode. Any other error fails the test.
fn sync_outcomes(path: &std::path::Path) -> Vec<Option<TestFact>> {
    FactStreamReader::<String, TestValue, String>::open(path)
        .unwrap()
        .map(|item| match item {
            Ok(fact) => Some(fact),
            Err(ReadError::Deserialization(_)) => None,
            Err(e) => panic!("expected a decode error, got {e:?}"),
        })
        .collect()
}

#[test]
fn sync_write_times_out_with_lock_timeout_when_lock_is_held() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let timeout = std::time::Duration::from_millis(100);
    let mut writer = FactStreamWriter::open_with_timeout(path, timeout).unwrap();
    let holder = external_lock(path);

    let result = writer.write_batch(&[numbered(1)]);
    assert!(
        matches!(result, Err(WriteError::LockTimeout(t)) if t == timeout),
        "expected LockTimeout, got {result:?}"
    );

    FileExt::unlock(&holder).unwrap();
    // The failed attempt did not leave the lock held.
    writer.write_batch(&[numbered(1)]).unwrap();
}

#[test]
fn sync_write_after_torn_tail_keeps_the_new_batch_intact() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let first = numbered(1);
    FactStreamWriter::open(path)
        .unwrap()
        .write_batch(std::slice::from_ref(&first))
        .unwrap();
    // A crashed writer left half a line behind.
    append_raw(path, br#"{"e":"torn"#);

    let batch = vec![numbered(2), numbered(3)];
    let position = FactStreamWriter::open(path)
        .unwrap()
        .write_batch(&batch)
        .unwrap();

    assert_eq!(position, file_len(path));
    assert_eq!(
        sync_outcomes(path),
        vec![
            Some(first),
            None,
            Some(batch[0].clone()),
            Some(batch[1].clone())
        ]
    );
}

#[cfg(feature = "tokio")]
async fn async_outcomes(path: &std::path::Path) -> Vec<Option<TestFact>> {
    let mut reader = AsyncFactStreamReader::<String, TestValue, String>::open(path)
        .await
        .unwrap();
    let mut items = Vec::new();
    while let Some(item) = reader.next().await {
        items.push(match item {
            Ok(fact) => Some(fact),
            Err(ReadError::Deserialization(_)) => None,
            Err(e) => panic!("expected a decode error, got {e:?}"),
        });
    }
    items
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_write_times_out_with_lock_timeout_when_lock_is_held() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let timeout = std::time::Duration::from_millis(100);
    let mut writer = AsyncFactStreamWriter::open_with_timeout(path, timeout)
        .await
        .unwrap();
    let holder = external_lock(path);

    let result = writer.write_batch(&[numbered(1)]).await;
    assert!(
        matches!(result, Err(WriteError::LockTimeout(t)) if t == timeout),
        "expected LockTimeout, got {result:?}"
    );

    FileExt::unlock(&holder).unwrap();
    writer.write_batch(&[numbered(1)]).await.unwrap();
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_write_after_torn_tail_keeps_the_new_batch_intact() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let first = numbered(1);
    AsyncFactStreamWriter::open(path)
        .await
        .unwrap()
        .write_batch(std::slice::from_ref(&first))
        .await
        .unwrap();
    append_raw(path, br#"{"e":"torn"#);

    let batch = vec![numbered(2), numbered(3)];
    let position = AsyncFactStreamWriter::open(path)
        .await
        .unwrap()
        .write_batch(&batch)
        .await
        .unwrap();

    assert_eq!(position, file_len(path));
    assert_eq!(
        async_outcomes(path).await,
        vec![
            Some(first),
            None,
            Some(batch[0].clone()),
            Some(batch[1].clone())
        ]
    );
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_write_batch_cancelled_while_waiting_for_the_lock_does_not_block_others() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let mut w1 = AsyncFactStreamWriter::open_with_timeout(path, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    let holder = external_lock(path);

    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        w1.write_batch(&[numbered(1)]),
    )
    .await;
    assert!(cancelled.is_err(), "expected the write to still be waiting");

    FileExt::unlock(&holder).unwrap();

    // The started write is not abandoned: it takes the lock once it is
    // free and lands. W2 waits for it rather than racing it.
    let mut w2 = AsyncFactStreamWriter::open_with_timeout(path, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    w2.write_batch(&[numbered(2)]).await.unwrap();
    // Either may have taken the lock first; wait (bounded) for both.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut written = loop {
        let written = async_read_all_at(path, StreamPosition::START).await;
        if written.len() >= 2 || std::time::Instant::now() >= deadline {
            break written;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    written.sort_by_key(|f| f.entity().clone());
    assert_eq!(written, vec![numbered(1), numbered(2)]);
    // W1 is still alive: its handle must not be holding the lock.
    let other = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    other.try_lock_exclusive().expect("lock still held");
    FileExt::unlock(&other).unwrap();
    drop(w1);
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_write_futures_are_send() {
    fn assert_send<T: Send>(_: &T) {}
    let temp = NamedTempFile::new().unwrap();
    let mut writer = AsyncFactStreamWriter::open(temp.path()).await.unwrap();
    let batch = vec![numbered(1)];
    {
        let write = writer.write_batch(&batch);
        assert_send(&write);
    }
    let end = writer.end_position();
    assert_send(&end);
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_write_batch_cancelled_after_acquiring_the_lock_releases_it() {
    use std::future::Future as _;
    use std::task::{Context, Waker};

    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let mut w1 = AsyncFactStreamWriter::open(path).await.unwrap();
    let batch = vec![numbered(1)];

    {
        // One poll starts the write; the future is then dropped mid-write.
        let mut write = Box::pin(w1.write_batch(&batch));
        let mut cx = Context::from_waker(Waker::noop());
        let _ = write.as_mut().poll(&mut cx);
    }

    // The started write runs to completion and then releases the lock.
    let other = wait_for_lock_after_write(path).await;
    FileExt::unlock(&other).unwrap();

    let mut w2 = AsyncFactStreamWriter::open(path).await.unwrap();
    w2.write_batch(&[numbered(2)]).await.unwrap();
    drop(w1);
}

/// Wait (bounded) until `path` is non-empty and a separate handle can take
/// its exclusive lock, and return that handle, still holding the lock.
#[cfg(feature = "tokio")]
async fn wait_for_lock_after_write(path: &std::path::Path) -> std::fs::File {
    let other = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if std::fs::metadata(path).unwrap().len() > 0 && other.try_lock_exclusive().is_ok() {
            return other;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the cancelled write never completed and released the lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_write_batch_dropped_after_first_poll_still_lands_completely() {
    use std::future::Future as _;
    use std::task::{Context, Waker};

    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let mut w1 = AsyncFactStreamWriter::open(path).await.unwrap();
    // Larger than tokio's background write chunk, so a write cut short
    // after the first chunk would leave a torn batch.
    let big = "x".repeat(1 << 20);
    let batch: Vec<TestFact> = (0..4)
        .map(|n| {
            fact(
                &format!("track{n}"),
                TestValue::Description(big.clone()),
                "2024-01-15T10:00:00Z",
                "alice",
            )
        })
        .collect();

    {
        let mut write = Box::pin(w1.write_batch(&batch));
        let mut cx = Context::from_waker(Waker::noop());
        let _ = write.as_mut().poll(&mut cx);
    }

    let other = wait_for_lock_after_write(path).await;
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(
        bytes.last(),
        Some(&b'\n'),
        "torn tail after cancelled write"
    );
    assert_eq!(async_read_all_at(path, StreamPosition::START).await, batch);
    FileExt::unlock(&other).unwrap();
    drop(w1);
}

#[cfg(feature = "tokio")]
async fn async_read_all_at(path: &std::path::Path, at: StreamPosition) -> Vec<TestFact> {
    let mut reader = AsyncFactStreamReader::open_at(path, at).await.unwrap();
    let mut facts = Vec::new();
    while let Some(result) = reader.next().await {
        facts.push(result.unwrap());
    }
    facts
}

#[cfg(feature = "tokio")]
async fn async_write_three_batches(
    path: &std::path::Path,
) -> (Vec<Vec<TestFact>>, Vec<StreamPosition>) {
    let batches = vec![
        vec![numbered(1)],
        vec![numbered(2), numbered(3)],
        vec![numbered(4), numbered(5), numbered(6)],
    ];
    let mut writer = AsyncFactStreamWriter::open(path).await.unwrap();
    let mut positions = Vec::new();
    for batch in &batches {
        positions.push(writer.write_batch(batch).await.unwrap());
    }
    (batches, positions)
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_reopen_at_each_batch_position_yields_exactly_the_later_facts() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let (batches, positions) = async_write_three_batches(path).await;

    assert!(positions[0] < positions[1] && positions[1] < positions[2]);
    assert_eq!(*positions.last().unwrap(), file_len(path));

    for (i, at) in positions.iter().enumerate() {
        let expected: Vec<TestFact> = batches[i + 1..].concat();
        assert_eq!(async_read_all_at(path, *at).await, expected, "after {i}");
    }
    assert_eq!(
        async_read_all_at(path, StreamPosition::START).await,
        batches.concat()
    );
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_position_at_eof_equals_write_batch_position_and_file_length() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let (_, positions) = async_write_three_batches(path).await;

    let mut reader = AsyncFactStreamReader::<String, TestValue, String>::open(path)
        .await
        .unwrap();
    assert_eq!(reader.position(), StreamPosition::START);
    while let Some(result) = reader.next().await {
        result.unwrap();
    }
    assert_eq!(reader.position(), *positions.last().unwrap());
    assert_eq!(reader.position(), file_len(path));
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_open_at_start_behaves_like_open() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let (batches, _) = async_write_three_batches(path).await;

    let mut reader = AsyncFactStreamReader::<String, TestValue, String>::open(path)
        .await
        .unwrap();
    let mut from_open = Vec::new();
    while let Some(result) = reader.next().await {
        from_open.push(result.unwrap());
    }
    assert_eq!(from_open, batches.concat());
    assert_eq!(
        async_read_all_at(path, StreamPosition::START).await,
        from_open
    );
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_partial_trailing_line_is_not_yielded_until_terminated() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let first = numbered(1);
    let second = numbered(2);
    let after_first = AsyncFactStreamWriter::open(path)
        .await
        .unwrap()
        .write_batch(std::slice::from_ref(&first))
        .await
        .unwrap();
    append_raw(path, second.to_line().unwrap().as_bytes());

    let mut reader = AsyncFactStreamReader::<String, TestValue, String>::open(path)
        .await
        .unwrap();
    assert_eq!(reader.next().await.unwrap().unwrap(), first);
    assert!(reader.next().await.is_none());
    assert_eq!(reader.position(), after_first);
    assert!(reader.next().await.is_none());
    assert_eq!(reader.position(), after_first);

    append_raw(path, b"\n");
    assert_eq!(reader.next().await.unwrap().unwrap(), second);
    assert_eq!(reader.position(), file_len(path));
    assert!(reader.next().await.is_none());
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_empty_lines_and_decode_errors_advance_the_position() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let first = numbered(1);
    let second = numbered(2);
    append_raw(
        path,
        format!("{}\n\n  \n", first.to_line().unwrap()).as_bytes(),
    );
    append_raw(path, b"garbage\n");
    append_raw(
        path,
        format!("\n{}\n\n", second.to_line().unwrap()).as_bytes(),
    );

    let mut reader = AsyncFactStreamReader::<String, TestValue, String>::open(path)
        .await
        .unwrap();
    assert_eq!(reader.next().await.unwrap().unwrap(), first);
    assert!(matches!(
        reader.next().await,
        Some(Err(ReadError::Deserialization(_)))
    ));
    let after_garbage = reader.position();
    assert_eq!(reader.next().await.unwrap().unwrap(), second);
    assert!(reader.next().await.is_none());
    assert_eq!(reader.position(), file_len(path));

    assert_eq!(async_read_all_at(path, after_garbage).await, vec![second]);
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_open_at_mid_line_or_past_eof_is_invalid_position() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();
    let (_, positions) = async_write_three_batches(path).await;

    let mid_line = StreamPosition::from_offset(positions[0].offset() + 3);
    let past_eof = StreamPosition::from_offset(positions[2].offset() + 1);
    for bad in [mid_line, past_eof] {
        let result = AsyncFactStreamReader::<String, TestValue, String>::open_at(path, bad).await;
        assert!(
            matches!(result, Err(ReadError::InvalidPosition(p)) if p == bad),
            "expected InvalidPosition({bad:?})"
        );
    }
    assert!(async_read_all_at(path, positions[2]).await.is_empty());
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_read_lines_at_pages_through_complete_lines() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let lines: Vec<String> = (1..=4).map(|n| numbered(n).to_line().unwrap()).collect();
    append_raw(
        path,
        format!("{}\n\n{}\n  \n{}\n", lines[0], lines[1], lines[2]).as_bytes(),
    );
    append_raw(path, lines[3].as_bytes());

    let (page1, at1) = read_lines_at_async(path, StreamPosition::START, 2)
        .await
        .unwrap();
    assert_eq!(page1, lines[0..2]);

    let (page2, at2) = read_lines_at_async(path, at1, 5).await.unwrap();
    assert_eq!(page2, lines[2..3]);
    assert_eq!(
        at2,
        StreamPosition::from_offset(file_len(path).offset() - lines[3].len() as u64)
    );

    append_raw(path, b"\n");
    let (page3, at3) = read_lines_at_async(path, at2, 5).await.unwrap();
    assert_eq!(page3, lines[3..4]);
    assert_eq!(at3, file_len(path));

    let bad = read_lines_at_async(path, StreamPosition::from_offset(1), 5).await;
    assert!(matches!(bad, Err(ReadError::InvalidPosition(_))));
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_writer_output_is_byte_identical_to_to_line_plus_newline() {
    let temp = NamedTempFile::new().unwrap();
    let path = temp.path();

    let facts = vec![fact(
        "track1",
        TestValue::Title("A\nB".to_string()),
        "2024-01-15T10:00:00Z",
        "alice",
    )];

    {
        let mut writer = AsyncFactStreamWriter::open(path).await.unwrap();
        writer.write_batch(&facts).await.unwrap();
    }

    let expected = format!("{}\n", facts[0].to_line().unwrap());
    assert_eq!(std::fs::read(path).unwrap(), expected.into_bytes());
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
