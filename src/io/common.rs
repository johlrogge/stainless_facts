use crate::Fact;
use serde::Serialize;

/// Serialize a batch of facts to a buffer.
///
/// All facts are serialized as newline-delimited JSON. Newlines within
/// fact values are automatically escaped by serde_json.
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
        serde_json::to_writer(&mut buffer, fact)?;
        buffer.push(b'\n');
    }

    Ok(buffer)
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
