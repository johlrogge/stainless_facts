//! The line codec: how a [`Fact`] is written to and read from a fact stream.
//!
//! The format itself is documented on [`Fact::to_line`].

use crate::Fact;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};

impl<E: Serialize, V: Serialize, S: Serialize> Fact<E, V, S> {
    /// Encode this fact as one line of the storage format, without a
    /// trailing newline.
    ///
    /// # Storage format
    ///
    /// This is the crate's storage-format contract. A fact stream is
    /// newline-delimited JSON: one fact per line, each line a JSON array of
    /// exactly five elements,
    ///
    /// ```text
    /// [entity, {"t": .., "v": ..}, timestamp, source, op]
    /// ```
    ///
    /// where the value is the attribute serialized with
    /// `#[serde(tag = "t", content = "v")]`, the timestamp is RFC 3339 and
    /// `op` is `"Assert"` or `"Retract"`. Newlines inside values are escaped
    /// by JSON, so a fact always occupies exactly one line.
    ///
    /// `to_line`, [`Fact::from_line`] and [`peek_entity`]
    /// are the supported way to produce and consume this format. The stream
    /// writers and readers (with the `io` feature) use the same code, so a
    /// writer's output is byte-identical to `to_line` followed by `'\n'`.
    ///
    /// ```
    /// use serde::Serialize;
    /// use stainless_facts::{Fact, Operation};
    ///
    /// #[derive(Serialize)]
    /// #[serde(tag = "t", content = "v")]
    /// enum Attribute {
    ///     Title(String),
    /// }
    ///
    /// let fact = Fact::new(
    ///     "track-1".to_string(),
    ///     Attribute::Title("Line 1\nLine 2".to_string()),
    ///     "2024-01-01T00:00:00Z".parse().unwrap(),
    ///     "alice".to_string(),
    ///     Operation::Assert,
    /// );
    /// let line = fact.to_line()?;
    /// assert_eq!(
    ///     line,
    ///     r#"["track-1",{"t":"Title","v":"Line 1\nLine 2"},"2024-01-01T00:00:00Z","alice","Assert"]"#
    /// );
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Append this fact's line and a terminating `'\n'` to `buf`.
    ///
    /// This is what the stream writers use, so their output is
    /// [`to_line`](Self::to_line) plus `'\n'` by construction. On error,
    /// `buf` may contain a partial line.
    #[cfg_attr(not(feature = "io"), allow(dead_code))]
    pub(crate) fn encode_into(&self, buf: &mut Vec<u8>) -> Result<(), serde_json::Error> {
        // `to_string` is `to_writer` into a fresh buffer, so this is the same
        // bytes as `to_line`, without the intermediate `String`.
        serde_json::to_writer(&mut *buf, self)?;
        buf.push(b'\n');
        Ok(())
    }
}

impl<E: DeserializeOwned, V: DeserializeOwned, S: DeserializeOwned> Fact<E, V, S> {
    /// Decode one line of the storage format.
    ///
    /// Surrounding whitespace, including a trailing `"\n"` or `"\r\n"`, is
    /// ignored. See [`Fact::to_line`] for the format.
    ///
    /// ```
    /// use stainless_facts::Fact;
    ///
    /// let line = "[\"track-1\",\"Song\",\"2024-01-01T00:00:00Z\",\"alice\",\"Assert\"]\r\n";
    /// let fact: Fact<String, String, String> = Fact::from_line(line)?;
    /// assert_eq!(fact.entity(), "track-1");
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        Self::decode_bytes(line.as_bytes())
    }

    /// Decode one line given as raw bytes, ignoring surrounding ASCII
    /// whitespace.
    pub(crate) fn decode_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes.trim_ascii())
    }
}

/// The entity of a line, with the other four elements skipped unparsed.
///
/// Keeping all five positions preserves the arity check: a line with more
/// or fewer elements is rejected, as it would be by [`Fact::from_line`].
#[derive(Deserialize)]
struct Head<E>(E, IgnoredAny, IgnoredAny, IgnoredAny, IgnoredAny);

/// Decode only the entity of a line in the storage format.
///
/// The value, timestamp, source and operation are skipped without being
/// decoded, so this works without knowing the line's value type (for
/// example to route lines by entity). The line must still be a well-formed
/// JSON array of exactly five elements. Surrounding whitespace is ignored,
/// as in [`Fact::from_line`].
///
/// ```
/// use stainless_facts::peek_entity;
///
/// let line = r#"["track-1",{"t":"Rating","v":5},"2024-01-01T00:00:00Z","v2","Assert"]"#;
/// let entity: String = peek_entity(line)?;
/// assert_eq!(entity, "track-1");
/// # Ok::<(), serde_json::Error>(())
/// ```
pub fn peek_entity<E: DeserializeOwned>(line: &str) -> Result<E, serde_json::Error> {
    let Head(entity, ..) = serde_json::from_slice::<Head<E>>(line.as_bytes().trim_ascii())?;
    Ok(entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Operation;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "t", content = "v")]
    enum Value {
        Title(String),
        Plays(u16),
    }

    type TestFact = Fact<String, Value, String>;

    fn fact(entity: &str, value: Value, operation: Operation) -> TestFact {
        Fact::new(
            entity.to_string(),
            value,
            "2024-01-15T10:00:00Z".parse().unwrap(),
            "alice".to_string(),
            operation,
        )
    }

    #[test]
    fn to_line_and_from_line_round_trip() {
        let originals = [
            fact("t1", Value::Title("Song".into()), Operation::Assert),
            fact("t2", Value::Plays(3), Operation::Retract),
        ];
        for original in originals {
            let line = original.to_line().unwrap();
            assert_eq!(TestFact::from_line(&line).unwrap(), original);
        }
    }

    #[test]
    fn to_line_matches_the_documented_format() {
        let line = fact("t1", Value::Plays(3), Operation::Assert)
            .to_line()
            .unwrap();
        assert_eq!(
            line,
            r#"["t1",{"t":"Plays","v":3},"2024-01-15T10:00:00Z","alice","Assert"]"#
        );
    }

    #[test]
    fn value_with_newlines_stays_on_one_line() {
        let original = fact(
            "t1",
            Value::Title("Line 1\nLine 2\r\nLine 3".into()),
            Operation::Assert,
        );
        let line = original.to_line().unwrap();

        assert!(!line.contains('\n'));
        assert!(!line.contains('\r'));
        assert_eq!(TestFact::from_line(&line).unwrap(), original);
    }

    #[test]
    fn encode_into_appends_to_line_plus_newline() {
        let first = fact("t1", Value::Title("a\nb".into()), Operation::Assert);
        let second = fact("t2", Value::Plays(1), Operation::Retract);

        let mut buf = Vec::new();
        first.encode_into(&mut buf).unwrap();
        second.encode_into(&mut buf).unwrap();

        let expected = format!(
            "{}\n{}\n",
            first.to_line().unwrap(),
            second.to_line().unwrap()
        );
        assert_eq!(buf, expected.into_bytes());
    }

    #[test]
    fn from_line_tolerates_trailing_crlf_and_surrounding_whitespace() {
        let original = fact("t1", Value::Plays(3), Operation::Assert);
        let line = original.to_line().unwrap();

        for padded in [
            format!("{line}\r\n"),
            format!("{line}\n"),
            format!("  \t{line} \r\n"),
        ] {
            assert_eq!(TestFact::from_line(&padded).unwrap(), original);
        }
    }

    #[test]
    fn from_line_rejects_wrong_arity_and_invalid_json() {
        for bad in [
            r#"["t1",{"t":"Plays","v":3},"2024-01-15T10:00:00Z","alice"]"#,
            r#"["t1",{"t":"Plays","v":3},"2024-01-15T10:00:00Z","alice","Assert",1]"#,
            "not json",
            "",
        ] {
            assert!(TestFact::from_line(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn peek_entity_works_without_knowing_the_value_type() {
        // `Value` has no `Rating` variant, so a full decode would fail.
        let line = r#"["t1",{"t":"Rating","v":{"stars":5}},"2024-01-15T10:00:00Z","bob","Assert"]"#;
        assert!(TestFact::from_line(line).is_err());

        let entity: String = peek_entity(line).unwrap();
        assert_eq!(entity, "t1");
    }

    #[test]
    fn peek_entity_decodes_structured_entities_and_trims_whitespace() {
        let line = "[[1,2],{\"t\":\"X\"},\"2024-01-15T10:00:00Z\",\"bob\",\"Retract\"]\r\n";
        let entity: (u32, u32) = peek_entity(line).unwrap();
        assert_eq!(entity, (1, 2));
    }

    #[test]
    fn peek_entity_rejects_wrong_arity() {
        for bad in [
            r#"["t1"]"#,
            r#"["t1",{"t":"X"},"2024-01-15T10:00:00Z","bob"]"#,
            r#"["t1",{"t":"X"},"2024-01-15T10:00:00Z","bob","Assert","extra"]"#,
        ] {
            assert!(peek_entity::<String>(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn peek_entity_rejects_invalid_json_and_wrong_entity_type() {
        assert!(peek_entity::<String>("not json").is_err());
        let line = r#"[42,{"t":"X"},"2024-01-15T10:00:00Z","bob","Assert"]"#;
        assert!(peek_entity::<String>(line).is_err());
    }
}
