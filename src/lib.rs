use chrono::{DateTime, Utc};
// src/lib.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Assert,
    Retract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact<E, V>(E, V, DateTime<Utc>, String, Operation);

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Bpm(u16);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    // #[serde(tag = "t", content = "d")]
    enum TestValue {
        Bpm(Bpm),
        Title(String),
    }

    type TestFact = Fact<String, TestValue>;

    #[rstest]
    #[case::test_numeric(
        r#"("some_song", Title("a_title"), "2024-01-15T10:30:00Z", "alice", Assert)"#,
        Ok(Fact(
            "some_song".to_string(),
            TestValue::Title("a_title".to_string()),
            "2024-01-15T10:30:00Z".parse().unwrap(),
            "alice".to_string(), Operation::Assert)))
]
    fn deserialize(
        #[case] serialized: &str,
        #[case] expected: Result<TestFact, ron::de::SpannedError>,
    ) {
        let actual: Result<TestFact, ron::de::SpannedError> = ron::from_str(serialized);
        assert_eq!(expected, actual);
    }
}
