//! # Fact Stream
//!
//! A simple, immutable fact stream library inspired by Datomic, designed for systems
//! that need schema evolution and time-travel capabilities.
//!
//! By default, this library includes synchronous I/O with `FactStore`. Enable the `tokio`
//! feature for async I/O with `AsyncFactStore`.

// Sync I/O - always available
pub mod io;
pub mod store;

pub use io::{FactStreamReader, FactStreamWriter, ReadError, WriteError};
pub use store::{FactIterator, FactStore, StoreError};

// Async I/O - only with tokio feature
#[cfg(feature = "tokio")]
mod async_store;

#[cfg(feature = "tokio")]
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

        match fact.operation() {
            Operation::Assert => aggregator.assert(fact.value(), fact.source()),
            Operation::Retract => aggregator.retract(fact.value(), fact.source()),
        }
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
