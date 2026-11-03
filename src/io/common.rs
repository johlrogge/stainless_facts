use crate::Fact;
use serde::Serialize;

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
/// without its trailing `'\n'`.
pub(crate) fn line_to_string(line: &[u8]) -> std::io::Result<String> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    String::from_utf8(line.to_vec())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Operation;

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
