use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, hash::Hash};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Assert,
    Retract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact<E, V, S>(E, V, DateTime<Utc>, S, Operation);

impl<E, V, S> Fact<E, V, S> {
    pub fn new(
        entity: E,
        value: V,
        timestamp: DateTime<Utc>,
        source: S,
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

    pub fn source(&self) -> &S {
        &self.3
    }

    pub fn operation(&self) -> Operation {
        self.4
    }
}

// Trait now includes S
pub trait FactAggregator<E, V, S> {
    fn assert(&mut self, value: &V, source: &S);
    fn retract(&mut self, value: &V, source: &S);
}

// Aggregation function updated
pub fn aggregate_facts<E, V, S, A, I>(facts: I) -> HashMap<E, A>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S> + Default,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    let mut aggregators = HashMap::<E, A>::new();

    for fact in facts {
        let aggregator: &mut A = aggregators.entry(fact.entity().clone()).or_default();

        match fact.operation() {
            Operation::Assert => aggregator.assert(fact.value(), fact.source()),
            Operation::Retract => aggregator.retract(fact.value(), fact.source()),
        }
    }

    aggregators
}

// Convenience type aliases
pub type StringSourceFact<E, V> = Fact<E, V, String>;
pub type NoSourceFact<E, V> = Fact<E, V, ()>;

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

    type TestFact = StringSourceFact<String, TestValue>;

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
