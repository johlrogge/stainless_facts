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
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
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
///
/// Equivalent to [`aggregate_facts_into`] on an empty map, so a whole fold
/// and a fold resumed at any split point produce the same map.
///
/// A Retract against an entity with no aggregate is a no-op: an entity whose
/// facts in `facts` are only retracts gets no entry (before 0.3.0 it got a
/// `Default` entry).
pub fn aggregate_facts<E, V, S, A, I>(facts: I) -> HashMap<E, A>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S> + Default,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    let mut aggregates = HashMap::new();
    aggregate_facts_into(&mut aggregates, facts);
    aggregates
}

/// Fold `facts` (in log order) into existing `aggregates`. Returns the
/// entities touched.
///
/// This resumes a fold: for any split of a fact sequence into a prefix and a
/// suffix, folding the suffix into `aggregate_facts(prefix)` gives the same
/// map as `aggregate_facts` over the whole sequence. New entities start from
/// `A::default()`.
///
/// A Retract against an entity with no aggregate is a no-op: no entry is
/// created and the entity is not touched.
///
/// # Touched set
///
/// The returned set contains exactly the keys whose map state this call
/// changed: inserted, mutated (a fact was dispatched to the aggregate), or
/// removed. A touched key that is absent from the map afterwards has been
/// retired. A Retract against an absent key is excluded, because nothing
/// changed. Keys not in the set were left exactly as they were, so a caller
/// only needs to revisit (for example with [`retain_touched`]) the touched
/// keys.
pub fn aggregate_facts_into<E, V, S, A, I>(aggregates: &mut HashMap<E, A>, facts: I) -> HashSet<E>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S> + Default,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    fold_into_with(aggregates, facts, |_| A::default())
}

/// The single fold behind every public aggregation entry point.
///
/// Per fact, in order: dispatch to the existing aggregate if there is one;
/// otherwise skip a Retract (no insert, no `init`, not touched) or insert
/// `init(&key)` for an Assert and dispatch to it. Every dispatched key is
/// recorded in the returned touched set.
fn fold_into_with<E, V, S, A, I>(
    aggregates: &mut HashMap<E, A>,
    facts: I,
    mut init: impl FnMut(&E) -> A,
) -> HashSet<E>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S>,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    let mut touched = HashSet::new();

    for fact in facts {
        let aggregate = match aggregates.entry(fact.entity().clone()) {
            Entry::Occupied(slot) => slot.into_mut(),
            Entry::Vacant(_) if fact.operation() == Operation::Retract => continue,
            Entry::Vacant(slot) => {
                let fresh = init(slot.key());
                slot.insert(fresh)
            }
        };
        aggregate.apply(&fact);

        if !touched.contains(fact.entity()) {
            touched.insert(fact.entity().clone());
        }
    }

    touched
}

/// Drop touched aggregates failing `keep`; the sweep cost scales with the
/// delta, not with the size of the map.
///
/// `keep` is called once for each key in `touched` that is present in
/// `aggregates`, with the key and its aggregate; entries for which it returns
/// `false` are removed. Entries whose key is not in `touched` are never
/// visited or removed. Touched keys that are absent from the map (retired
/// entities, see [`aggregate_facts_into`]) are skipped.
pub fn retain_touched<E: Eq + Hash, A>(
    aggregates: &mut HashMap<E, A>,
    touched: &HashSet<E>,
    mut keep: impl FnMut(&E, &A) -> bool,
) {
    for key in touched {
        if let Some(aggregate) = aggregates.get(key) {
            if !keep(key, aggregate) {
                aggregates.remove(key);
            }
        }
    }
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

    #[derive(Debug, Clone, Default, PartialEq)]
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

    fn titles(items: &[&str]) -> Titles {
        Titles(items.iter().map(|s| s.to_string()).collect())
    }

    fn keys(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn aggregate_facts_omits_retract_only_entities() {
        let facts = vec![
            fact("a", "one", Operation::Assert),
            fact("ghost", "one", Operation::Retract),
        ];

        let aggregates: HashMap<String, Titles> = aggregate_facts(facts);

        assert!(aggregates.contains_key("a"));
        assert!(!aggregates.contains_key("ghost"));
        assert_eq!(aggregates.len(), 1);
    }

    #[test]
    fn retract_against_absent_entity_creates_no_key() {
        let mut aggregates: HashMap<String, Titles> = HashMap::new();
        aggregates.insert("b".to_string(), titles(&["x"]));

        let touched =
            aggregate_facts_into(&mut aggregates, vec![fact("a", "one", Operation::Retract)]);

        assert!(!aggregates.contains_key("a"));
        assert_eq!(aggregates.len(), 1);
        assert!(touched.is_empty());
    }

    #[test]
    fn empty_delta_returns_empty_touched_set_and_leaves_map_unchanged() {
        let mut aggregates: HashMap<String, Titles> = HashMap::new();
        aggregates.insert("a".to_string(), titles(&["one"]));
        aggregates.insert("b".to_string(), titles(&[]));
        let before = aggregates.clone();

        let touched = aggregate_facts_into(&mut aggregates, Vec::new());

        assert!(touched.is_empty());
        assert_eq!(aggregates, before);
    }

    #[test]
    fn touched_set_is_delta_entities_minus_absent_retract_only_entities() {
        let mut aggregates: HashMap<String, Titles> = HashMap::new();
        aggregates.insert("present".to_string(), titles(&["one"]));
        aggregates.insert("untouched".to_string(), titles(&["one"]));

        let delta = vec![
            // present entity, retract only: touched
            fact("present", "one", Operation::Retract),
            // absent entity, retract only: not touched
            fact("ghost", "one", Operation::Retract),
            // absent entity, assert: touched
            fact("new", "one", Operation::Assert),
            // absent entity, retract then assert: touched (by the assert)
            fact("late", "one", Operation::Retract),
            fact("late", "two", Operation::Assert),
        ];

        let touched = aggregate_facts_into(&mut aggregates, delta);

        assert_eq!(touched, keys(&["present", "new", "late"]));
        assert_eq!(aggregates["present"], titles(&[]));
        assert_eq!(aggregates["new"], titles(&["one"]));
        assert_eq!(aggregates["late"], titles(&["two"]));
        assert_eq!(aggregates["untouched"], titles(&["one"]));
        assert!(!aggregates.contains_key("ghost"));
    }

    #[test]
    fn retain_touched_never_removes_untouched_entries() {
        let mut aggregates: HashMap<String, Titles> = HashMap::new();
        aggregates.insert("a".to_string(), titles(&[]));
        aggregates.insert("b".to_string(), titles(&[]));
        aggregates.insert("c".to_string(), titles(&["keep"]));
        // "gone" is touched but absent from the map; must be tolerated.
        let touched = keys(&["a", "c", "gone"]);

        let mut seen = Vec::new();
        retain_touched(&mut aggregates, &touched, |key, _| {
            seen.push(key.clone());
            false
        });

        assert_eq!(aggregates.len(), 1);
        assert!(aggregates.contains_key("b"));
        seen.sort();
        assert_eq!(seen, vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn retain_touched_keeps_entries_passing_keep() {
        let mut aggregates: HashMap<String, Titles> = HashMap::new();
        aggregates.insert("a".to_string(), titles(&[]));
        aggregates.insert("c".to_string(), titles(&["keep"]));
        let touched = keys(&["a", "c"]);

        retain_touched(&mut aggregates, &touched, |_, agg| !agg.0.is_empty());

        assert_eq!(aggregates.len(), 1);
        assert_eq!(aggregates["c"], titles(&["keep"]));
    }

    mod split_fold {
        use super::*;
        use proptest::prelude::*;

        const ENTITIES: [&str; 3] = ["a", "b", "c"];
        const TITLES: [&str; 2] = ["x", "y"];

        fn facts_strategy() -> impl Strategy<Value = Vec<Fact<String, TestValue, String>>> {
            prop::collection::vec((0..ENTITIES.len(), 0..TITLES.len(), any::<bool>()), 0..24)
                .prop_map(|raw| {
                    raw.into_iter()
                        .map(|(e, t, is_assert)| {
                            let op = if is_assert {
                                Operation::Assert
                            } else {
                                Operation::Retract
                            };
                            fact(ENTITIES[e], TITLES[t], op)
                        })
                        .collect()
                })
        }

        /// Entities in `delta`, minus those that only see retracts while
        /// absent from `before`.
        fn expected_touched(
            before: &HashMap<String, Titles>,
            delta: &[Fact<String, TestValue, String>],
        ) -> HashSet<String> {
            delta
                .iter()
                .filter(|f| before.contains_key(f.entity()) || f.operation() == Operation::Assert)
                .map(|f| f.entity().clone())
                .collect()
        }

        proptest! {
            #[test]
            fn split_fold_equals_whole_fold_at_every_split_point(facts in facts_strategy()) {
                let whole: HashMap<String, Titles> = aggregate_facts(facts.clone());

                for k in 0..=facts.len() {
                    let (prefix, suffix) = facts.split_at(k);
                    let mut resumed: HashMap<String, Titles> = aggregate_facts(prefix.to_vec());
                    let before = resumed.clone();

                    let touched = aggregate_facts_into(&mut resumed, suffix.to_vec());

                    prop_assert_eq!(&resumed, &whole, "split point k = {}", k);
                    prop_assert_eq!(touched, expected_touched(&before, suffix), "split point k = {}", k);
                    for (key, agg) in &resumed {
                        if !expected_touched(&before, suffix).contains(key) {
                            prop_assert_eq!(Some(agg), before.get(key));
                        }
                    }
                }
            }
        }
    }
}
