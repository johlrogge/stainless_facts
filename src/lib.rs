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

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Copy)]
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

#[cfg(test)]
mod aggregation_tests {
    use super::*;
    use rstest::rstest;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Copy)]
    #[serde(transparent)]
    struct Bpm(u16);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(transparent)]
    struct Tag(String);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum TestValue {
        Bpm(Bpm),
        Title(String),
        Tag(Tag),
    }

    #[derive(Default, Debug, PartialEq)]
    struct TestAggregation {
        bpm: Option<Bpm>,
        title: Option<String>,
        tags: Vec<Tag>,
    }

    impl FactAggregator<String, TestValue, String> for TestAggregation {
        fn assert(&mut self, value: &TestValue, _source: &String) {
            match value {
                TestValue::Bpm(bpm) => self.bpm = Some(*bpm),
                TestValue::Title(title) => self.title = Some(title.clone()),
                TestValue::Tag(tag) => {
                    if !self.tags.contains(tag) {
                        self.tags.push(tag.clone());
                    }
                }
            }
        }

        fn retract(&mut self, value: &TestValue, _source: &String) {
            match value {
                TestValue::Bpm(bpm) => {
                    if self.bpm.as_ref() == Some(bpm) {
                        self.bpm = None;
                    }
                }
                TestValue::Title(title) => {
                    if self.title.as_ref() == Some(title) {
                        self.title = None;
                    }
                }
                TestValue::Tag(tag) => {
                    self.tags.retain(|t| t != tag);
                }
            }
        }
    }

    #[rstest]
    #[case::single_entity_single_fact(
        vec![
            Fact::new(
                "track1".to_string(),
                TestValue::Bpm(Bpm(12800)),
                "2024-01-15T10:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert
            )
        ],
        {
            let mut expected = HashMap::new();
            expected.insert("track1".to_string(), TestAggregation {
                bpm: Some(Bpm(12800)),
                ..Default::default()
            });
            expected
        }
    )]
    #[case::cardinality_one_latest_wins(
        vec![
            Fact::new(
                "track1".to_string(),
                TestValue::Bpm(Bpm(12800)),
                "2024-01-15T10:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert
            ),
            Fact::new(
                "track1".to_string(),
                TestValue::Bpm(Bpm(13000)),
                "2024-01-15T11:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert
            )
        ],
        {
            let mut expected = HashMap::new();
            expected.insert("track1".to_string(), TestAggregation {
                bpm: Some(Bpm(13000)), // Latest wins
                ..Default::default()
            });
            expected
        }
    )]
    #[case::cardinality_many_accumulates(
        vec![
            Fact::new(
                "track1".to_string(),
                TestValue::Tag(Tag("techno".to_string())),
                "2024-01-15T10:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert
            ),
            Fact::new(
                "track1".to_string(),
                TestValue::Tag(Tag("minimal".to_string())),
                "2024-01-15T11:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert
            )
        ],
        {
            let mut expected = HashMap::new();
            expected.insert("track1".to_string(), TestAggregation {
                tags: vec![Tag("techno".to_string()), Tag("minimal".to_string())],
                ..Default::default()
            });
            expected
        }
    )]
    #[case::retract_removes_tag(
        vec![
            Fact::new(
                "track1".to_string(),
                TestValue::Tag(Tag("techno".to_string())),
                "2024-01-15T10:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert
            ),
            Fact::new(
                "track1".to_string(),
                TestValue::Tag(Tag("minimal".to_string())),
                "2024-01-15T11:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert
            ),
            Fact::new(
                "track1".to_string(),
                TestValue::Tag(Tag("techno".to_string())),
                "2024-01-15T12:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Retract
            )
        ],
        {
            let mut expected = HashMap::new();
            expected.insert("track1".to_string(), TestAggregation {
                tags: vec![Tag("minimal".to_string())],
                ..Default::default()
            });
            expected
        }
    )]
    #[case::multiple_entities(
        vec![
            Fact::new(
                "track1".to_string(),
                TestValue::Title("First Track".to_string()),
                "2024-01-15T10:00:00Z".parse().unwrap(),
                "alice".to_string(),
                Operation::Assert
            ),
            Fact::new(
                "track2".to_string(),
                TestValue::Title("Second Track".to_string()),
                "2024-01-15T11:00:00Z".parse().unwrap(),
                "bob".to_string(),
                Operation::Assert
            )
        ],
        {
            let mut expected = HashMap::new();
            expected.insert("track1".to_string(), TestAggregation {
                title: Some("First Track".to_string()),
                ..Default::default()
            });
            expected.insert("track2".to_string(), TestAggregation {
                title: Some("Second Track".to_string()),
                ..Default::default()
            });
            expected
        }
    )]
    fn aggregate_facts_test(
        #[case] facts: Vec<Fact<String, TestValue, String>>,
        #[case] expected: HashMap<String, TestAggregation>,
    ) {
        let result: HashMap<String, TestAggregation> = aggregate_facts(facts);
        assert_eq!(expected, result);
    }
}
