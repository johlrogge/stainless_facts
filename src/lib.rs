// src/lib.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::hash::Hash;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownAttribute {
    pub t: String,
    pub v: JsonValue,
}

pub trait FactAggregator<E, V, S> {
    fn assert(&mut self, value: &V, source: &S);
    fn retract(&mut self, value: &V, source: &S);

    // Default implementations that do nothing
    fn assert_unknown(&mut self, _attribute: &str, _value: &JsonValue, _source: &S) {}
    fn retract_unknown(&mut self, _attribute: &str, _value: &JsonValue, _source: &S) {}
}

pub fn aggregate_facts<E, V, S, A, I>(facts: I) -> HashMap<E, A>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S> + Default,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    let mut aggregators = HashMap::new();

    for fact in facts {
        let aggregator: &mut A = aggregators.entry(fact.entity().clone()).or_default();

        match fact.operation() {
            Operation::Assert => aggregator.assert(fact.value(), fact.source()),
            Operation::Retract => aggregator.retract(fact.value(), fact.source()),
        }
    }

    aggregators
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(transparent)]
    struct Bpm(u16);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(transparent)]
    struct Tag(String);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "t", content = "v")]
    enum TestValue {
        Bpm(Bpm),
        Title(String),
        Tag(Tag),
    }

    type TestFact = Fact<String, TestValue, String>;

    #[test]
    fn see_what_json_produces() {
        let fact = Fact::new(
            "some_song".to_string(),
            TestValue::Title("a_title".to_string()),
            "2024-01-15T10:30:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert,
        );

        let serialized = serde_json::to_string(&fact).unwrap();
        println!("JSON produces: {}", serialized);

        // Try round-trip
        let deserialized: Result<TestFact, _> = serde_json::from_str(&serialized);
        assert!(deserialized.is_ok());
    }

    #[rstest]
    #[case::string_variant(
        r#"["some_song",{"t":"Title","v":"a_title"},"2024-01-15T10:30:00Z","alice","Assert"]"#,
        Ok(Fact::new(
            "some_song".to_string(),
            TestValue::Title("a_title".to_string()),
            "2024-01-15T10:30:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert
        ))
    )]
    #[case::transparent_new_type_variant(
        r#"["some_song",{"t":"Bpm","v":12350},"2024-01-16T10:30:00Z","alice","Assert"]"#,
        Ok(Fact::new(
            "some_song".to_string(),
            TestValue::Bpm(Bpm(12350)),
            "2024-01-16T10:30:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert
        ))
    )]
    fn deserialize(
        #[case] serialized: &str,
        #[case] expected: Result<TestFact, serde_json::Error>,
    ) {
        let actual: Result<TestFact, serde_json::Error> = serde_json::from_str(serialized);
        match actual {
            Ok(a) => match expected {
                Ok(e) => assert_eq!(e, a),
                Err(e) => panic!("got error {e}"),
            },
            Err(e) => {
                if let Ok(a) = expected {
                    panic!("expected {a:?} but got error {e}")
                }
            }
        }
    }

    #[test]
    fn deserialize_unknown_attribute() {
        let serialized = r#"["some_song",{"t":"NewAttribute","v":"some_value"},"2024-01-15T10:30:00Z","alice","Assert"]"#;

        let result: Result<Fact<String, UnknownAttribute, String>, _> =
            serde_json::from_str(serialized);
        assert!(result.is_ok());

        let fact = result.unwrap();
        assert_eq!(fact.entity(), "some_song");
        assert_eq!(fact.value().t, "NewAttribute");
        assert_eq!(fact.value().v, JsonValue::String("some_value".to_string()));
    }

    #[test]
    fn deserialize_unknown_attribute_with_number() {
        let serialized =
            r#"["some_song",{"t":"NewAttribute","v":42},"2024-01-15T10:30:00Z","alice","Assert"]"#;

        let result: Result<Fact<String, UnknownAttribute, String>, _> =
            serde_json::from_str(serialized);
        assert!(result.is_ok());

        let fact = result.unwrap();
        assert_eq!(fact.entity(), "some_song");
        assert_eq!(fact.value().t, "NewAttribute");
        assert_eq!(fact.value().v, JsonValue::Number(42.into()));
    }
}

#[cfg(test)]
mod aggregation_tests {
    use super::*;
    use rstest::rstest;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(transparent)]
    struct Bpm(u16);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(transparent)]
    struct Tag(String);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "t", content = "v")]
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
        unknown_attributes: HashMap<String, JsonValue>,
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

        fn assert_unknown(&mut self, attribute: &str, value: &JsonValue, _source: &String) {
            self.unknown_attributes
                .insert(attribute.to_string(), value.clone());
        }

        fn retract_unknown(&mut self, attribute: &str, _value: &JsonValue, _source: &String) {
            self.unknown_attributes.remove(attribute);
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
                bpm: Some(Bpm(13000)),
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
