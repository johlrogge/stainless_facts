//! # Fact Stream
//!
//! A simple, immutable fact stream library inspired by Datomic, designed for systems
//! that need schema evolution and time-travel capabilities.
//!
//! By default, this library provides only the core fact types and aggregation logic,
//! with no I/O dependencies. Enable the `io` feature for synchronous file I/O with
//! `FactStore`, or the `tokio` feature (which implies `io`) for async I/O with
//! `AsyncFactStore`.

#![cfg_attr(docsrs, feature(doc_cfg))]

// Sync I/O - only with io feature
#[cfg(feature = "io")]
#[cfg_attr(docsrs, doc(cfg(feature = "io")))]
pub mod io;
#[cfg(feature = "io")]
#[cfg_attr(docsrs, doc(cfg(feature = "io")))]
pub mod store;

#[cfg(feature = "io")]
#[cfg_attr(docsrs, doc(cfg(feature = "io")))]
pub use io::{FactStreamReader, FactStreamWriter, ReadError, WriteError};
#[cfg(feature = "io")]
#[cfg_attr(docsrs, doc(cfg(feature = "io")))]
pub use store::{FactIterator, FactStore, StoreError};

// Async I/O - only with tokio feature
#[cfg(feature = "tokio")]
mod async_store;

#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub use async_store::{AsyncFactIterator, AsyncFactStore};

// Core types
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::hash::Hash;

/// Represents whether a fact is asserting or retracting information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    /// Assert a value - add or update an attribute
    Assert,
    /// Retract a value - remove an attribute value
    Retract,
}

/// A fact represents a single assertion or retraction about an entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact<E, V, S>(E, V, DateTime<Utc>, S, Operation);

impl<E, V, S> Fact<E, V, S> {
    /// Create a new fact.
    pub fn new(
        entity: E,
        value: V,
        timestamp: DateTime<Utc>,
        source: S,
        operation: Operation,
    ) -> Self {
        Self(entity, value, timestamp, source, operation)
    }

    /// Get a reference to the entity this fact is about.
    pub fn entity(&self) -> &E {
        &self.0
    }

    /// Get a reference to the value (attribute and its data).
    pub fn value(&self) -> &V {
        &self.1
    }

    /// Get a reference to the timestamp when this fact was recorded.
    pub fn timestamp(&self) -> &DateTime<Utc> {
        &self.2
    }

    /// Get a reference to the source that created this fact.
    pub fn source(&self) -> &S {
        &self.3
    }

    /// Get the operation (Assert or Retract).
    pub fn operation(&self) -> Operation {
        self.4
    }
}

/// Represents an unknown attribute that wasn't recognized during deserialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownAttribute {
    /// The attribute name as a string
    pub t: String,
    /// The value as a generic JSON value
    pub v: JsonValue,
}

/// Trait for aggregating facts into domain-specific data structures.
pub trait FactAggregator<E, V, S> {
    /// Handle an assertion fact.
    fn assert(&mut self, value: &V, source: &S);

    /// Handle a retraction fact.
    fn retract(&mut self, value: &V, source: &S);

    /// Apply a single fact to this aggregator.
    ///
    /// This is the single entry point the fold functions (such as
    /// [`aggregate_facts`]) call for every fact, and the only hook that sees
    /// the whole [`Fact`]: entity, value, timestamp, source and operation.
    /// The default implementation dispatches on [`Fact::operation`] to
    /// [`assert`](Self::assert) or [`retract`](Self::retract).
    ///
    /// Override it when the aggregate needs the timestamp (e.g. last-write-wins),
    /// the entity, or wants to handle both operations in one place.
    ///
    /// Overriding `apply` does not make `assert` and `retract` optional; they
    /// remain required methods. This is deliberate: if they had default no-op
    /// bodies, an implementation that forgot to override anything would
    /// compile silently and drop every fact. An `apply` override that does not
    /// use them may implement them as `unreachable!()` or delegate to them.
    fn apply(&mut self, fact: &Fact<E, V, S>) {
        match fact.operation() {
            Operation::Assert => self.assert(fact.value(), fact.source()),
            Operation::Retract => self.retract(fact.value(), fact.source()),
        }
    }

    /// Handle an unknown assertion.
    fn assert_unknown(&mut self, _attribute: &str, _value: &JsonValue, _source: &S) {}

    /// Handle an unknown retraction.
    fn retract_unknown(&mut self, _attribute: &str, _value: &JsonValue, _source: &S) {}
}

/// Aggregate an iterator of facts into a map of aggregated entities.
pub fn aggregate_facts<E, V, S, A, I>(facts: I) -> HashMap<E, A>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S> + Default,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    let mut aggregators = HashMap::new();

    for fact in facts {
        let aggregator: &mut A = aggregators.entry(fact.entity().clone()).or_default();
        aggregator.apply(&fact);
    }

    aggregators
}

/// Trait for building validated output from an aggregator.
pub trait Buildable {
    type Output;
    type Error;

    fn build(self) -> Result<Self::Output, Self::Error>;
}

/// Aggregate facts and build validated output.
pub fn aggregate_and_build<E, V, S, A, I>(facts: I) -> Result<HashMap<E, A::Output>, A::Error>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S> + Default + Buildable,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    let aggregators: HashMap<E, A> = aggregate_facts(facts);

    aggregators
        .into_iter()
        .map(|(entity, aggregator)| aggregator.build().map(|output| (entity, output)))
        .collect()
}

/// Validates that a value type uses the correct serialization format.
///
/// Values must use `#[serde(tag = "t", content = "v")]` for proper serialization.
#[macro_export]
macro_rules! assert_fact_value_format {
    ($value:expr) => {{
        let json = serde_json::to_value(&$value).expect("Failed to serialize value");
        let obj = json.as_object().expect("Value must serialize to an object");
        assert!(obj.contains_key("t"), "Value must have a 't' (tag) field");
        assert!(
            obj.contains_key("v"),
            "Value must have a 'v' (content) field"
        );
        assert_eq!(
            obj.len(),
            2,
            "Value must have exactly 2 fields: 't' and 'v'"
        );
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "t", content = "v")]
    enum TestValue {
        Title(String),
    }

    #[derive(Default)]
    struct Titles(Vec<String>);

    impl FactAggregator<String, TestValue, String> for Titles {
        fn assert(&mut self, value: &TestValue, _source: &String) {
            let TestValue::Title(title) = value;
            self.0.push(title.clone());
        }

        fn retract(&mut self, value: &TestValue, _source: &String) {
            let TestValue::Title(title) = value;
            self.0.retain(|t| t != title);
        }
    }

    fn fact(entity: &str, title: &str, operation: Operation) -> Fact<String, TestValue, String> {
        Fact::new(
            entity.to_string(),
            TestValue::Title(title.to_string()),
            Utc::now(),
            "test".to_string(),
            operation,
        )
    }

    #[test]
    fn aggregate_facts_folds_asserts_and_retracts_per_entity() {
        let facts = vec![
            fact("a", "one", Operation::Assert),
            fact("b", "two", Operation::Assert),
            fact("a", "three", Operation::Assert),
            fact("a", "one", Operation::Retract),
        ];

        let aggregates: HashMap<String, Titles> = aggregate_facts(facts);

        assert_eq!(aggregates["a"].0, vec!["three".to_string()]);
        assert_eq!(aggregates["b"].0, vec!["two".to_string()]);
    }

    fn fact_at(
        entity: &str,
        title: &str,
        secs: i64,
        operation: Operation,
    ) -> Fact<String, TestValue, String> {
        Fact::new(
            entity.to_string(),
            TestValue::Title(title.to_string()),
            DateTime::from_timestamp(secs, 0).expect("valid timestamp"),
            "test".to_string(),
            operation,
        )
    }

    /// Records everything `apply` sees; assert/retract must never be reached.
    #[derive(Default)]
    struct Recorder(Vec<(String, DateTime<Utc>, Operation)>);

    impl FactAggregator<String, TestValue, String> for Recorder {
        fn assert(&mut self, _value: &TestValue, _source: &String) {
            panic!("apply override should bypass assert");
        }

        fn retract(&mut self, _value: &TestValue, _source: &String) {
            panic!("apply override should bypass retract");
        }

        fn apply(&mut self, fact: &Fact<String, TestValue, String>) {
            self.0
                .push((fact.entity().clone(), *fact.timestamp(), fact.operation()));
        }
    }

    #[test]
    fn default_apply_matches_assert_retract_dispatch_on_mixed_stream() {
        let facts = vec![
            fact("a", "one", Operation::Assert),
            fact("a", "two", Operation::Assert),
            fact("b", "x", Operation::Assert),
            fact("a", "one", Operation::Retract),
            fact("b", "y", Operation::Assert),
            fact("b", "x", Operation::Retract),
        ];

        let mut expected: HashMap<String, Titles> = HashMap::new();
        for f in facts.clone() {
            let agg = expected.entry(f.entity().clone()).or_default();
            match f.operation() {
                Operation::Assert => agg.assert(f.value(), f.source()),
                Operation::Retract => agg.retract(f.value(), f.source()),
            }
        }

        let actual: HashMap<String, Titles> = aggregate_facts(facts);

        assert_eq!(actual.len(), expected.len());
        for (k, v) in &expected {
            assert_eq!(actual[k].0, v.0);
        }
        assert_eq!(actual["a"].0, vec!["two".to_string()]);
        assert_eq!(actual["b"].0, vec!["y".to_string()]);
    }

    #[test]
    fn overriding_apply_sees_timestamps_in_order() {
        let facts = vec![
            fact_at("a", "one", 10, Operation::Assert),
            fact_at("a", "two", 20, Operation::Assert),
            fact_at("a", "three", 30, Operation::Assert),
        ];

        let aggregates: HashMap<String, Recorder> = aggregate_facts(facts);

        let seen: Vec<i64> = aggregates["a"]
            .0
            .iter()
            .map(|(_, t, _)| t.timestamp())
            .collect();
        assert_eq!(seen, vec![10, 20, 30]);
    }

    #[test]
    fn entity_passed_to_apply_matches_map_key() {
        let facts = vec![
            fact_at("a", "one", 1, Operation::Assert),
            fact_at("b", "two", 2, Operation::Assert),
            fact_at("a", "three", 3, Operation::Assert),
        ];

        let aggregates: HashMap<String, Recorder> = aggregate_facts(facts);

        for (key, rec) in &aggregates {
            assert!(!rec.0.is_empty());
            for (entity, _, _) in &rec.0 {
                assert_eq!(entity, key);
            }
        }
    }

    #[test]
    fn retract_reaches_apply_as_retract_operation() {
        let facts = vec![
            fact_at("a", "one", 1, Operation::Assert),
            fact_at("a", "one", 2, Operation::Retract),
        ];

        let aggregates: HashMap<String, Recorder> = aggregate_facts(facts);

        let ops: Vec<Operation> = aggregates["a"].0.iter().map(|(_, _, op)| *op).collect();
        assert_eq!(ops, vec![Operation::Assert, Operation::Retract]);
    }

    #[test]
    fn value_format_is_tagged() {
        assert_fact_value_format!(TestValue::Title("x".to_string()));
    }
}
