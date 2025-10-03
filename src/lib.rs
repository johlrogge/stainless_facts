use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Assert,
    Retract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact<E, V>(E, V, DateTime<Utc>, String, Operation);

impl<E, V> Fact<E, V> {
    pub fn new(
        entity: E,
        value: V,
        timestamp: DateTime<Utc>,
        source: String,
        operation: Operation,
    ) -> Self {
        Self(entity, value, timestamp, source, operation)
    }

    pub fn entity(&self) -> &E {
        &self.0
    }

    pub fn value(&self) -> &V {
        &self.1
    }

    pub fn timestamp(&self) -> &DateTime<Utc> {
        &self.2
    }

    pub fn source(&self) -> &str {
        &self.3
    }

    pub fn operation(&self) -> Operation {
        self.4
    }
}

// The trait
pub trait FactAggregator<E, V> {
    fn add_fact(&mut self, fact: &Fact<E, V>);
}

// The aggregation function
pub fn aggregate_facts<E, V, A, I>(facts: I) -> HashMap<E, A>
where
    E: Eq + std::hash::Hash + Clone,
    A: FactAggregator<E, V> + Default,
    I: IntoIterator<Item = Fact<E, V>>,
{
    let mut aggregators: HashMap<E, A> = HashMap::new();

    for fact in facts {
        aggregators
            .entry(fact.entity().clone())
            .or_default()
            .add_fact(&fact);
    }

    aggregators
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(transparent)]
    struct Bpm(u16);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum TestValue {
        Bpm(Bpm),
        Title(String),
    }

    type TestFact = Fact<String, TestValue>;

    #[rstest]
    #[case::string_variant(
        r#"("some_song", Title("a_title"), "2024-01-15T10:30:00Z", "alice", Assert)"#,
        Ok(Fact::new(
            "some_song".to_string(),
            TestValue::Title("a_title".to_string()),
            "2024-01-15T10:30:00Z".parse().unwrap(),
            "alice".to_string(), Operation::Assert)))]
    #[case::transparent_new_type_variant(
        r#"("some_song", Bpm(12350), "2024-01-16T10:30:00Z", "alice", Assert)"#,
        Ok(Fact::new(
            "some_song".to_string(),
            TestValue::Bpm(Bpm(12350)),
            "2024-01-16T10:30:00Z".parse().unwrap(),
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
