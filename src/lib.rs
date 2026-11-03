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
use serde::de::{DeserializeOwned, Deserializer, Error as _};
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

    /// Consume the fact and return its parts:
    /// `(entity, value, timestamp, source, operation)`.
    ///
    /// Use this to move the parts out without cloning them.
    pub fn into_parts(self) -> (E, V, DateTime<Utc>, S, Operation) {
        (self.0, self.1, self.2, self.3, self.4)
    }
}

/// Represents an unknown attribute that wasn't recognized during deserialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownAttribute {
    /// The attribute name as a string
    pub t: String,
    /// The value as a generic JSON value. A missing `v` (as written for a
    /// unit variant, `{"t":"X"}`) decodes as `null`.
    #[serde(default)]
    pub v: JsonValue,
}

/// An attribute value that is either one this reader knows (`V`) or one it
/// cannot decode, kept as an [`UnknownAttribute`].
///
/// This is how a reader tolerates facts from a newer writer (schema
/// evolution): deserialize the stream as [`OpenFact`] instead of [`Fact`] and
/// fold it with [`aggregate_open_facts_into`].
///
/// # Classification
///
/// Deserializing an `Attr<V>` reads the attribute JSON once, then:
///
/// 1. if `V` can decode it, it is [`Attr::Known`];
/// 2. otherwise, if it is an object with a string `t` (and an optional `v`,
///    which defaults to `null`), it is [`Attr::Unknown`];
/// 3. otherwise it is a deserialization error.
///
/// So `Unknown` means "this reader's `V` cannot decode it": that covers a tag
/// `V` has never heard of *and* a known tag whose payload changed type (for
/// example a `u16` field that now holds a string). Consumers that want such
/// changes to be errors should deserialize plain `Fact<E, V, S>` instead.
///
/// Only the attribute is lenient. A line that is not valid JSON, an outer
/// array that does not have exactly five elements, an entity, timestamp,
/// source or operation that does not decode, or an attribute that is not an
/// object with a string `t`, is still an error.
///
/// Serialization is untagged: `Known(v)` serializes as `v` and `Unknown(u)`
/// as `{"t": .., "v": ..}`, so an unknown fact is re-emitted as equivalent
/// JSON. Object keys nested inside `v` may come out in a different order;
/// relays that need byte-exact pass-through should forward raw lines instead
/// of decoding them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Attr<V> {
    /// An attribute this reader's `V` decodes.
    Known(V),
    /// An attribute this reader's `V` cannot decode.
    Unknown(UnknownAttribute),
}

impl<'de, V: DeserializeOwned> Deserialize<'de> for Attr<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const MALFORMED: &str = "attribute must be an object with a string `t`";

        let value = JsonValue::deserialize(deserializer)?;
        if let Ok(known) = V::deserialize(&value) {
            return Ok(Attr::Known(known));
        }
        // A derived struct also accepts a sequence (`["X", 1]`), so require an
        // object explicitly.
        if !value.is_object() {
            return Err(D::Error::custom(MALFORMED));
        }
        UnknownAttribute::deserialize(value)
            .map(Attr::Unknown)
            .map_err(|_| D::Error::custom(MALFORMED))
    }
}

/// A fact whose attribute may be unknown to this reader; see [`Attr`].
pub type OpenFact<E, V, S> = Fact<E, Attr<V>, S>;

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

    /// Apply a single fact whose attribute this reader could not decode.
    ///
    /// The open folds (such as [`aggregate_open_facts_into`]) call this for
    /// every [`Attr::Unknown`] fact, the way they call [`apply`](Self::apply)
    /// for every known one. The default implementation dispatches on
    /// [`Fact::operation`] to [`assert_unknown`](Self::assert_unknown) or
    /// [`retract_unknown`](Self::retract_unknown) with the attribute's `t`
    /// and `v`.
    ///
    /// Override it when handling an unknown attribute needs the timestamp or
    /// the entity.
    fn apply_unknown(&mut self, fact: &Fact<E, UnknownAttribute, S>) {
        let UnknownAttribute { t, v } = fact.value();
        match fact.operation() {
            Operation::Assert => self.assert_unknown(t, v, fact.source()),
            Operation::Retract => self.retract_unknown(t, v, fact.source()),
        }
    }

    /// Whether this aggregate has become empty and should be removed.
    ///
    /// The fold functions (such as [`aggregate_facts_into`]) check this after
    /// every Retract they dispatch to the aggregate, never after an Assert. If
    /// it returns `true`, the fold removes the entry from the map. The key
    /// still appears in the returned touched set, so callers see the change.
    /// A later Assert for the same entity recreates the aggregate from scratch
    /// (via `A::default()`, or the `init` passed to
    /// [`aggregate_facts_into_with`]).
    ///
    /// Override it to express "reset to empty when the last tracked fact is
    /// retracted". The default returns `false`, so entries are never removed.
    fn is_retired(&self) -> bool {
        false
    }
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
/// `A::default()`; use [`aggregate_facts_into_with`] to build them from their
/// key instead.
///
/// A Retract against an entity with no aggregate is a no-op: no entry is
/// created and the entity is not touched. After a Retract is dispatched, an
/// aggregate whose [`FactAggregator::is_retired`] returns `true` is removed
/// from the map; a later Assert recreates it from `A::default()`.
///
/// # Touched set
///
/// The returned set contains exactly the keys whose map state this call
/// changed: inserted, mutated (a fact was dispatched to the aggregate), or
/// removed. A touched key that is absent from the map afterwards has been
/// retired: a Retract left it reporting [`FactAggregator::is_retired`], and
/// no later Assert in `facts` recreated it. A Retract against an absent key
/// is excluded, because nothing changed. Keys not in the set were left
/// exactly as they were, so a caller only needs to revisit (for example with
/// [`retain_touched`]) the touched keys.
pub fn aggregate_facts_into<E, V, S, A, I>(aggregates: &mut HashMap<E, A>, facts: I) -> HashSet<E>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S> + Default,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    aggregate_facts_into_with(aggregates, facts, |_| A::default())
}

/// Like [`aggregate_facts_into`], but new entities are built by `init`, which
/// receives the entity key.
///
/// This works like [`Entry::or_insert_with_key`]: `init(&key)` is called only
/// when an Assert reaches an entity with no aggregate in the map. That covers
/// a brand-new entity and one that was retired earlier (see
/// [`FactAggregator::is_retired`]). It is not called for entities already in
/// the map, and not for a Retract against an absent entity, which is a no-op.
/// This lets an aggregate know its own entity without a placeholder
/// `Default` or a post-pass, and it does not require `A: Default`.
///
/// The returned touched set has the same contract as for
/// [`aggregate_facts_into`].
pub fn aggregate_facts_into_with<E, V, S, A, I>(
    aggregates: &mut HashMap<E, A>,
    facts: I,
    init: impl FnMut(&E) -> A,
) -> HashSet<E>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S>,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    fold_into_with::<E, V, S, A, _>(aggregates, facts, init)
}

/// Fold [`OpenFact`]s (in log order) into existing `aggregates`. Returns the
/// entities touched.
///
/// This is [`aggregate_facts_into`] for a stream that may contain attributes
/// this reader cannot decode (see [`Attr`]). [`Attr::Known`] facts reach
/// [`FactAggregator::apply`] exactly as in [`aggregate_facts_into`];
/// [`Attr::Unknown`] facts reach [`FactAggregator::apply_unknown`] (by
/// default [`assert_unknown`](FactAggregator::assert_unknown) or
/// [`retract_unknown`](FactAggregator::retract_unknown)).
///
/// Unknown facts follow the same entity rules as known ones: an unknown
/// Assert on an absent entity creates it from `A::default()`, an unknown
/// Retract on an absent entity is a no-op and not touched, and
/// [`FactAggregator::is_retired`] is checked after an unknown Retract too.
/// The touched set has the same contract as for [`aggregate_facts_into`].
///
/// # Example
///
/// A reader that only knows `Title` folds a stream where a newer writer has
/// added a `Rating` attribute. The new line degrades gracefully to
/// `assert_unknown` instead of failing to deserialize:
///
/// ```
/// use serde::{Deserialize, Serialize};
/// use serde_json::Value;
/// use stainless_facts::{aggregate_open_facts_into, FactAggregator, OpenFact};
/// use std::collections::HashMap;
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// #[serde(tag = "t", content = "v")]
/// enum Track {
///     Title(String),
/// }
///
/// #[derive(Default)]
/// struct Summary {
///     title: Option<String>,
///     unknown: Vec<(String, Value)>,
/// }
///
/// impl FactAggregator<String, Track, String> for Summary {
///     fn assert(&mut self, value: &Track, _source: &String) {
///         let Track::Title(title) = value;
///         self.title = Some(title.clone());
///     }
///     fn retract(&mut self, _value: &Track, _source: &String) {
///         self.title = None;
///     }
///     fn assert_unknown(&mut self, attribute: &str, value: &Value, _source: &String) {
///         self.unknown.push((attribute.to_owned(), value.clone()));
///     }
/// }
///
/// let lines = [
///     r#"["track-1",{"t":"Title","v":"Song"},"2024-01-01T00:00:00Z","v1","Assert"]"#,
///     // Written by a newer version that knows about ratings.
///     r#"["track-1",{"t":"Rating","v":5},"2024-01-02T00:00:00Z","v2","Assert"]"#,
/// ];
/// let facts = lines
///     .iter()
///     .map(|line| serde_json::from_str::<OpenFact<String, Track, String>>(line))
///     .collect::<Result<Vec<_>, _>>()?;
///
/// let mut aggregates: HashMap<String, Summary> = HashMap::new();
/// aggregate_open_facts_into(&mut aggregates, facts);
///
/// let track = &aggregates["track-1"];
/// assert_eq!(track.title.as_deref(), Some("Song"));
/// assert_eq!(track.unknown, vec![("Rating".to_owned(), Value::from(5))]);
/// # Ok::<(), serde_json::Error>(())
/// ```
pub fn aggregate_open_facts_into<E, V, S, A, I>(
    aggregates: &mut HashMap<E, A>,
    facts: I,
) -> HashSet<E>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S> + Default,
    I: IntoIterator<Item = OpenFact<E, V, S>>,
{
    aggregate_open_facts_into_with(aggregates, facts, |_| A::default())
}

/// Like [`aggregate_open_facts_into`], but new entities are built by `init`,
/// which receives the entity key; see [`aggregate_facts_into_with`].
///
/// `init` is called only when an Assert, known or unknown, reaches an entity
/// with no aggregate in the map.
pub fn aggregate_open_facts_into_with<E, V, S, A, I>(
    aggregates: &mut HashMap<E, A>,
    facts: I,
    init: impl FnMut(&E) -> A,
) -> HashSet<E>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S>,
    I: IntoIterator<Item = OpenFact<E, V, S>>,
{
    fold_into_with::<E, V, S, A, _>(aggregates, facts, init)
}

mod sealed {
    use super::{Attr, Fact, FactAggregator, Operation};

    /// An item the fold can consume: a known [`Fact`] or an open fact.
    ///
    /// Private, so the known and open folds share exactly one fold body and
    /// differ only in how an item reaches the aggregate.
    pub trait FoldItem<E, V, S> {
        fn entity(&self) -> &E;
        fn operation(&self) -> Operation;
        fn dispatch<A: FactAggregator<E, V, S>>(self, aggregate: &mut A);
    }

    impl<E, V, S> FoldItem<E, V, S> for Fact<E, V, S> {
        fn entity(&self) -> &E {
            Fact::entity(self)
        }

        fn operation(&self) -> Operation {
            Fact::operation(self)
        }

        fn dispatch<A: FactAggregator<E, V, S>>(self, aggregate: &mut A) {
            aggregate.apply(&self);
        }
    }

    impl<E, V, S> FoldItem<E, V, S> for Fact<E, Attr<V>, S> {
        fn entity(&self) -> &E {
            Fact::entity(self)
        }

        fn operation(&self) -> Operation {
            Fact::operation(self)
        }

        fn dispatch<A: FactAggregator<E, V, S>>(self, aggregate: &mut A) {
            let (entity, value, timestamp, source, operation) = self.into_parts();
            match value {
                Attr::Known(value) => {
                    aggregate.apply(&Fact::new(entity, value, timestamp, source, operation))
                }
                Attr::Unknown(unknown) => aggregate
                    .apply_unknown(&Fact::new(entity, unknown, timestamp, source, operation)),
            }
        }
    }
}

use sealed::FoldItem;

/// The single fold behind every public aggregation entry point, known and
/// open alike.
///
/// Per item, in order: dispatch to the existing aggregate if there is one;
/// otherwise skip a Retract (no insert, no `init`, not touched) or insert
/// `init(&key)` for an Assert and dispatch to it. Every dispatched key is
/// recorded in the returned touched set. After a dispatched Retract (known or
/// unknown), an aggregate reporting [`FactAggregator::is_retired`] is
/// removed; its key stays touched.
fn fold_into_with<E, V, S, A, T>(
    aggregates: &mut HashMap<E, A>,
    items: impl IntoIterator<Item = T>,
    mut init: impl FnMut(&E) -> A,
) -> HashSet<E>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S>,
    T: FoldItem<E, V, S>,
{
    let mut touched = HashSet::new();

    for item in items {
        let key = item.entity().clone();
        let is_retract = item.operation() == Operation::Retract;
        let aggregate = match aggregates.entry(key.clone()) {
            Entry::Occupied(slot) => slot.into_mut(),
            Entry::Vacant(_) if is_retract => continue,
            Entry::Vacant(slot) => {
                let fresh = init(slot.key());
                slot.insert(fresh)
            }
        };
        item.dispatch(aggregate);
        let retired = is_retract && aggregate.is_retired();

        if retired {
            aggregates.remove(&key);
        }
        touched.insert(key);
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

    /// Knows its own entity (set by `init`) and retires when emptied by a
    /// retract.
    #[derive(Debug, Clone, PartialEq)]
    struct Keyed {
        key: String,
        titles: Vec<String>,
    }

    impl Keyed {
        fn new(key: &str) -> Self {
            Keyed {
                key: key.to_owned(),
                titles: Vec::new(),
            }
        }
    }

    impl FactAggregator<String, TestValue, String> for Keyed {
        fn assert(&mut self, value: &TestValue, _source: &String) {
            let TestValue::Title(title) = value;
            self.titles.push(title.clone());
        }

        fn retract(&mut self, value: &TestValue, _source: &String) {
            let TestValue::Title(title) = value;
            self.titles.retain(|t| t != title);
        }

        fn is_retired(&self) -> bool {
            self.titles.is_empty()
        }
    }

    #[test]
    fn init_is_called_exactly_once_per_new_entity_with_its_key() {
        let mut aggregates: HashMap<String, Keyed> = HashMap::new();
        let mut calls = Vec::new();

        let facts = vec![
            fact("a", "one", Operation::Assert),
            fact("b", "two", Operation::Assert),
            fact("a", "three", Operation::Assert),
            fact("b", "four", Operation::Assert),
        ];
        aggregate_facts_into_with(&mut aggregates, facts, |key| {
            calls.push(key.clone());
            Keyed::new(key)
        });

        calls.sort();
        assert_eq!(calls, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(aggregates["a"].key, "a");
        assert_eq!(aggregates["b"].key, "b");
        assert_eq!(aggregates["a"].titles, vec!["one", "three"]);
    }

    #[test]
    fn init_is_not_called_for_entities_already_in_the_map() {
        let mut aggregates: HashMap<String, Keyed> = HashMap::new();
        aggregates.insert("a".to_string(), Keyed::new("a"));
        let mut calls = Vec::new();

        aggregate_facts_into_with(
            &mut aggregates,
            vec![
                fact("a", "one", Operation::Assert),
                fact("a", "two", Operation::Assert),
            ],
            |key| {
                calls.push(key.clone());
                Keyed::new(key)
            },
        );

        assert!(calls.is_empty());
        assert_eq!(aggregates["a"].titles, vec!["one", "two"]);
    }

    #[test]
    fn init_is_not_called_when_a_retract_hits_an_absent_entity() {
        let mut aggregates: HashMap<String, Keyed> = HashMap::new();
        let mut calls = Vec::new();

        let touched = aggregate_facts_into_with(
            &mut aggregates,
            vec![fact("ghost", "one", Operation::Retract)],
            |key| {
                calls.push(key.clone());
                Keyed::new(key)
            },
        );

        assert!(calls.is_empty());
        assert!(aggregates.is_empty());
        assert!(touched.is_empty());
    }

    #[test]
    fn retired_entity_is_recreated_fresh_by_init_on_next_assert() {
        let mut aggregates: HashMap<String, Keyed> = HashMap::new();
        let mut calls = Vec::new();

        aggregate_facts_into_with(
            &mut aggregates,
            vec![
                fact("a", "one", Operation::Assert),
                fact("a", "one", Operation::Retract),
                fact("a", "two", Operation::Assert),
            ],
            |key| {
                calls.push(key.clone());
                Keyed::new(key)
            },
        );

        assert_eq!(calls, vec!["a".to_string(), "a".to_string()]);
        assert_eq!(
            aggregates["a"],
            Keyed {
                key: "a".to_string(),
                titles: vec!["two".to_string()],
            }
        );
    }

    #[test]
    fn retired_entity_is_touched_and_absent_from_the_map() {
        let mut aggregates: HashMap<String, Keyed> = HashMap::new();
        aggregates.insert(
            "a".to_string(),
            Keyed {
                key: "a".to_string(),
                titles: vec!["one".to_string()],
            },
        );

        let touched = aggregate_facts_into_with(
            &mut aggregates,
            vec![fact("a", "one", Operation::Retract)],
            |key| Keyed::new(key),
        );

        assert!(!aggregates.contains_key("a"));
        assert_eq!(touched, keys(&["a"]));
    }

    #[test]
    fn retirement_also_applies_through_aggregate_facts_into() {
        #[derive(Debug, Default)]
        struct Counter(u32);
        impl FactAggregator<String, TestValue, String> for Counter {
            fn assert(&mut self, _value: &TestValue, _source: &String) {
                self.0 += 1;
            }
            fn retract(&mut self, _value: &TestValue, _source: &String) {
                self.0 = self.0.saturating_sub(1);
            }
            fn is_retired(&self) -> bool {
                self.0 == 0
            }
        }

        let mut aggregates: HashMap<String, Counter> = HashMap::new();
        let touched = aggregate_facts_into(
            &mut aggregates,
            vec![
                fact("a", "one", Operation::Assert),
                fact("a", "one", Operation::Retract),
            ],
        );

        assert!(aggregates.is_empty());
        assert_eq!(touched, keys(&["a"]));
    }

    /// Always reports retired; used to show `is_retired` is not checked after
    /// an Assert.
    #[derive(Debug, Default)]
    struct AlwaysRetired(u32);

    impl FactAggregator<String, TestValue, String> for AlwaysRetired {
        fn assert(&mut self, _value: &TestValue, _source: &String) {
            self.0 += 1;
        }
        fn retract(&mut self, _value: &TestValue, _source: &String) {}
        fn is_retired(&self) -> bool {
            true
        }
    }

    #[test]
    fn aggregate_reporting_retired_after_an_assert_is_kept() {
        let mut aggregates: HashMap<String, AlwaysRetired> = HashMap::new();

        let touched = aggregate_facts_into(
            &mut aggregates,
            vec![
                fact("a", "one", Operation::Assert),
                fact("a", "two", Operation::Assert),
            ],
        );

        assert_eq!(aggregates["a"].0, 2);
        assert_eq!(touched, keys(&["a"]));

        // The same aggregate is removed as soon as a Retract reaches it.
        aggregate_facts_into(&mut aggregates, vec![fact("a", "one", Operation::Retract)]);
        assert!(!aggregates.contains_key("a"));
    }

    mod open_facts {
        use super::*;
        use rstest::rstest;
        use serde_json::json;

        /// The reader's view of the schema. A newer writer may emit tags this
        /// reader doesn't know, or change a payload type.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "t", content = "v")]
        enum Song {
            Title(String),
            Plays(u16),
            Favourite,
        }

        type Open = OpenFact<String, Song, String>;

        #[derive(Debug, Clone, PartialEq)]
        enum Seen {
            Known(Operation, Song),
            Unknown(Operation, String, JsonValue, String),
        }

        /// Records known and unknown dispatches in order, and retires when
        /// its live count drops to zero.
        #[derive(Debug, Default, PartialEq)]
        struct Log {
            seen: Vec<Seen>,
            live: i32,
        }

        impl FactAggregator<String, Song, String> for Log {
            fn assert(&mut self, value: &Song, _source: &String) {
                self.seen
                    .push(Seen::Known(Operation::Assert, value.clone()));
                self.live += 1;
            }

            fn retract(&mut self, value: &Song, _source: &String) {
                self.seen
                    .push(Seen::Known(Operation::Retract, value.clone()));
                self.live -= 1;
            }

            fn assert_unknown(&mut self, attribute: &str, value: &JsonValue, source: &String) {
                self.seen.push(Seen::Unknown(
                    Operation::Assert,
                    attribute.to_owned(),
                    value.clone(),
                    source.clone(),
                ));
                self.live += 1;
            }

            fn retract_unknown(&mut self, attribute: &str, value: &JsonValue, source: &String) {
                self.seen.push(Seen::Unknown(
                    Operation::Retract,
                    attribute.to_owned(),
                    value.clone(),
                    source.clone(),
                ));
                self.live -= 1;
            }
        }

        /// Same as `Log`, but retires when emptied by a retract.
        #[derive(Debug, Default, PartialEq)]
        struct RetiringLog(Log);

        impl FactAggregator<String, Song, String> for RetiringLog {
            fn assert(&mut self, value: &Song, source: &String) {
                self.0.assert(value, source);
            }

            fn retract(&mut self, value: &Song, source: &String) {
                self.0.retract(value, source);
            }

            fn assert_unknown(&mut self, attribute: &str, value: &JsonValue, source: &String) {
                self.0.assert_unknown(attribute, value, source);
            }

            fn retract_unknown(&mut self, attribute: &str, value: &JsonValue, source: &String) {
                self.0.retract_unknown(attribute, value, source);
            }

            fn is_retired(&self) -> bool {
                self.0.live <= 0
            }
        }

        fn line(entity: &str, attr: JsonValue, source: &str, op: &str) -> String {
            json!([entity, attr, "2024-01-01T00:00:00Z", source, op]).to_string()
        }

        fn parse(line: &str) -> Open {
            serde_json::from_str(line).expect("line should decode as an open fact")
        }

        fn unknown(t: &str, v: JsonValue) -> Attr<Song> {
            Attr::Unknown(UnknownAttribute { t: t.to_owned(), v })
        }

        #[test]
        fn mixed_stream_folds_known_and_routes_unknown_to_assert_unknown() {
            let facts = vec![
                parse(&line(
                    "a",
                    json!({"t": "Title", "v": "Song"}),
                    "v1",
                    "Assert",
                )),
                parse(&line("a", json!({"t": "Rating", "v": 5}), "v2", "Assert")),
                parse(&line("b", json!({"t": "Plays", "v": 3}), "v1", "Assert")),
                parse(&line(
                    "a",
                    json!({"t": "Tags", "v": ["x", "y"]}),
                    "v2",
                    "Assert",
                )),
                parse(&line(
                    "a",
                    json!({"t": "Title", "v": "Song"}),
                    "v1",
                    "Retract",
                )),
            ];

            let mut aggregates: HashMap<String, Log> = HashMap::new();
            let touched = aggregate_open_facts_into(&mut aggregates, facts);

            assert_eq!(touched, keys(&["a", "b"]));
            assert_eq!(
                aggregates["a"].seen,
                vec![
                    Seen::Known(Operation::Assert, Song::Title("Song".to_owned())),
                    Seen::Unknown(
                        Operation::Assert,
                        "Rating".to_owned(),
                        json!(5),
                        "v2".to_owned()
                    ),
                    Seen::Unknown(
                        Operation::Assert,
                        "Tags".to_owned(),
                        json!(["x", "y"]),
                        "v2".to_owned()
                    ),
                    Seen::Known(Operation::Retract, Song::Title("Song".to_owned())),
                ]
            );
            assert_eq!(
                aggregates["b"].seen,
                vec![Seen::Known(Operation::Assert, Song::Plays(3))]
            );
        }

        #[test]
        fn unknown_retract_reaches_retract_unknown() {
            let facts = vec![
                parse(&line("a", json!({"t": "Rating", "v": 5}), "v2", "Assert")),
                parse(&line("a", json!({"t": "Rating", "v": 5}), "v2", "Retract")),
            ];

            let mut aggregates: HashMap<String, Log> = HashMap::new();
            aggregate_open_facts_into(&mut aggregates, facts);

            assert_eq!(
                aggregates["a"].seen,
                vec![
                    Seen::Unknown(
                        Operation::Assert,
                        "Rating".to_owned(),
                        json!(5),
                        "v2".to_owned()
                    ),
                    Seen::Unknown(
                        Operation::Retract,
                        "Rating".to_owned(),
                        json!(5),
                        "v2".to_owned()
                    ),
                ]
            );
        }

        #[test]
        fn overriding_apply_unknown_sees_the_whole_fact() {
            #[derive(Default)]
            struct Stamps(Vec<(String, i64, Operation)>);
            impl FactAggregator<String, Song, String> for Stamps {
                fn assert(&mut self, _value: &Song, _source: &String) {}
                fn retract(&mut self, _value: &Song, _source: &String) {}
                fn apply_unknown(&mut self, fact: &Fact<String, UnknownAttribute, String>) {
                    self.0.push((
                        fact.entity().clone(),
                        fact.timestamp().timestamp(),
                        fact.operation(),
                    ));
                }
            }

            let ts = DateTime::from_timestamp(42, 0).expect("valid timestamp");
            let facts = vec![Fact::new(
                "a".to_owned(),
                unknown("Rating", json!(5)),
                ts,
                "v2".to_owned(),
                Operation::Assert,
            )];

            let mut aggregates: HashMap<String, Stamps> = HashMap::new();
            aggregate_open_facts_into(&mut aggregates, facts);

            assert_eq!(
                aggregates["a"].0,
                vec![("a".to_owned(), 42, Operation::Assert)]
            );
        }

        #[test]
        fn unknown_assert_on_absent_entity_creates_it_via_init() {
            let mut aggregates: HashMap<String, Log> = HashMap::new();
            let mut calls = Vec::new();

            let touched = aggregate_open_facts_into_with(
                &mut aggregates,
                vec![parse(&line(
                    "new",
                    json!({"t": "Rating", "v": 5}),
                    "v2",
                    "Assert",
                ))],
                |key| {
                    calls.push(key.clone());
                    Log::default()
                },
            );

            assert_eq!(calls, vec!["new".to_owned()]);
            assert_eq!(touched, keys(&["new"]));
            assert_eq!(aggregates["new"].seen.len(), 1);
        }

        #[test]
        fn unknown_retract_on_absent_entity_is_a_noop_and_not_touched() {
            let mut aggregates: HashMap<String, Log> = HashMap::new();
            let mut calls = Vec::new();

            let touched = aggregate_open_facts_into_with(
                &mut aggregates,
                vec![parse(&line(
                    "ghost",
                    json!({"t": "Rating", "v": 5}),
                    "v2",
                    "Retract",
                ))],
                |key| {
                    calls.push(key.clone());
                    Log::default()
                },
            );

            assert!(calls.is_empty());
            assert!(aggregates.is_empty());
            assert!(touched.is_empty());
        }

        #[test]
        fn is_retired_is_honoured_after_an_unknown_retract() {
            let mut aggregates: HashMap<String, RetiringLog> = HashMap::new();

            let touched = aggregate_open_facts_into(
                &mut aggregates,
                vec![
                    parse(&line(
                        "a",
                        json!({"t": "Title", "v": "Song"}),
                        "v1",
                        "Assert",
                    )),
                    parse(&line(
                        "a",
                        json!({"t": "Title", "v": "Song"}),
                        "v1",
                        "Retract",
                    )),
                ],
            );
            // Known retract retires it, as before.
            assert!(aggregates.is_empty());
            assert_eq!(touched, keys(&["a"]));

            let touched = aggregate_open_facts_into(
                &mut aggregates,
                vec![
                    parse(&line(
                        "b",
                        json!({"t": "Title", "v": "Song"}),
                        "v1",
                        "Assert",
                    )),
                    parse(&line("b", json!({"t": "Rating", "v": 5}), "v2", "Assert")),
                    parse(&line(
                        "b",
                        json!({"t": "Title", "v": "Song"}),
                        "v1",
                        "Retract",
                    )),
                ],
            );
            assert!(aggregates.contains_key("b"));
            assert_eq!(touched, keys(&["b"]));

            let touched = aggregate_open_facts_into(
                &mut aggregates,
                vec![parse(&line(
                    "b",
                    json!({"t": "Rating", "v": 5}),
                    "v2",
                    "Retract",
                ))],
            );
            assert!(!aggregates.contains_key("b"));
            assert_eq!(touched, keys(&["b"]));
        }

        #[test]
        fn unknown_assert_does_not_trigger_retirement() {
            let mut aggregates: HashMap<String, RetiringLog> = HashMap::new();
            aggregates.insert(
                "a".to_owned(),
                RetiringLog(Log {
                    seen: vec![],
                    live: -5,
                }),
            );

            aggregate_open_facts_into(
                &mut aggregates,
                vec![parse(&line(
                    "a",
                    json!({"t": "Rating", "v": 5}),
                    "v2",
                    "Assert",
                ))],
            );

            assert!(aggregates.contains_key("a"));
        }

        #[test]
        fn known_tags_decode_as_known() {
            let attr: Attr<Song> = serde_json::from_value(json!({"t": "Plays", "v": 3})).unwrap();
            assert_eq!(attr, Attr::Known(Song::Plays(3)));

            let attr: Attr<Song> = serde_json::from_value(json!({"t": "Favourite"})).unwrap();
            assert_eq!(attr, Attr::Known(Song::Favourite));
        }

        #[test]
        fn changed_payload_type_for_a_known_tag_decodes_as_unknown() {
            let attr: Attr<Song> =
                serde_json::from_value(json!({"t": "Plays", "v": "many"})).unwrap();
            assert_eq!(attr, unknown("Plays", json!("many")));

            let attr: Attr<Song> =
                serde_json::from_value(json!({"t": "Plays", "v": 70000})).unwrap();
            assert_eq!(attr, unknown("Plays", json!(70000)));
        }

        #[test]
        fn new_tag_decodes_as_unknown() {
            let attr: Attr<Song> =
                serde_json::from_value(json!({"t": "Rating", "v": {"stars": 5}})).unwrap();
            assert_eq!(attr, unknown("Rating", json!({"stars": 5})));
        }

        #[test]
        fn unknown_fact_round_trips_unchanged() {
            // Nested keys already in serde_json's canonical (sorted) order:
            // the round-trip is byte-identical.
            let original = r#"["a",{"t":"Rating","v":{"by":["x"],"stars":5}},"2024-01-01T00:00:00Z","v2","Assert"]"#;

            let fact = parse(original);
            assert_eq!(
                fact.value(),
                &unknown("Rating", json!({"stars": 5, "by": ["x"]}))
            );

            assert_eq!(serde_json::to_string(&fact).unwrap(), original);
        }

        #[test]
        fn unknown_fact_round_trips_to_equal_json_whatever_the_key_order() {
            // Without serde_json's `preserve_order` feature, nested object
            // keys are re-emitted sorted, so compare as JSON values.
            let original = r#"["a",{"t":"Rating","v":{"stars":5,"by":["x"]}},"2024-01-01T00:00:00Z","v2","Assert"]"#;

            let reencoded = serde_json::to_string(&parse(original)).unwrap();

            assert_eq!(
                serde_json::from_str::<JsonValue>(&reencoded).unwrap(),
                serde_json::from_str::<JsonValue>(original).unwrap()
            );
        }

        #[test]
        fn known_open_fact_serializes_like_a_plain_fact() {
            let original = r#"["a",{"t":"Plays","v":3},"2024-01-01T00:00:00Z","v1","Assert"]"#;

            let fact = parse(original);
            assert_eq!(fact.value(), &Attr::Known(Song::Plays(3)));

            assert_eq!(serde_json::to_string(&fact).unwrap(), original);
        }

        #[test]
        fn unknown_attribute_without_v_decodes_with_null_v() {
            let attr: Attr<Song> = serde_json::from_value(json!({"t": "Pinned"})).unwrap();
            assert_eq!(attr, unknown("Pinned", JsonValue::Null));

            let fact = parse(r#"["a",{"t":"Pinned"},"2024-01-01T00:00:00Z","v2","Assert"]"#);
            assert_eq!(fact.value(), &unknown("Pinned", JsonValue::Null));
        }

        #[rstest]
        #[case::invalid_json(r#"["a",{"t":"Title","v":"x"},"2024-01-01T00:00:00Z","v1","Assert""#)]
        #[case::not_json_at_all("not json")]
        #[case::too_few_elements(r#"["a",{"t":"Title","v":"x"},"2024-01-01T00:00:00Z","v1"]"#)]
        #[case::too_many_elements(
            r#"["a",{"t":"Title","v":"x"},"2024-01-01T00:00:00Z","v1","Assert","extra"]"#
        )]
        #[case::outer_not_an_array(r#"{"t":"Title","v":"x"}"#)]
        #[case::bad_entity(r#"[1,{"t":"Title","v":"x"},"2024-01-01T00:00:00Z","v1","Assert"]"#)]
        #[case::bad_timestamp(r#"["a",{"t":"Title","v":"x"},"yesterday","v1","Assert"]"#)]
        #[case::bad_source(r#"["a",{"t":"Title","v":"x"},"2024-01-01T00:00:00Z",7,"Assert"]"#)]
        #[case::bad_operation(
            r#"["a",{"t":"Title","v":"x"},"2024-01-01T00:00:00Z","v1","Upsert"]"#
        )]
        #[case::attribute_is_a_string(r#"["a","Title","2024-01-01T00:00:00Z","v1","Assert"]"#)]
        #[case::attribute_is_a_number(r#"["a",5,"2024-01-01T00:00:00Z","v1","Assert"]"#)]
        #[case::attribute_is_null(r#"["a",null,"2024-01-01T00:00:00Z","v1","Assert"]"#)]
        #[case::attribute_is_an_array(r#"["a",["Rating",5],"2024-01-01T00:00:00Z","v1","Assert"]"#)]
        #[case::attribute_without_t(r#"["a",{"v":5},"2024-01-01T00:00:00Z","v1","Assert"]"#)]
        #[case::attribute_with_non_string_t(
            r#"["a",{"t":5,"v":5},"2024-01-01T00:00:00Z","v1","Assert"]"#
        )]
        fn truly_malformed_lines_are_errors_not_unknown_attributes(#[case] line: &str) {
            let result = serde_json::from_str::<Open>(line);
            assert!(result.is_err(), "expected an error, got {result:?}");
        }

        #[test]
        fn malformed_attribute_error_names_the_requirement() {
            let err = serde_json::from_value::<Attr<Song>>(json!("Title")).unwrap_err();
            assert!(
                err.to_string()
                    .contains("attribute must be an object with a string `t`"),
                "unexpected error: {err}"
            );
        }

        #[test]
        fn into_parts_returns_all_fields_in_order() {
            let ts = DateTime::from_timestamp(7, 0).expect("valid timestamp");
            let fact = Fact::new("e", 1u8, ts, "s", Operation::Retract);

            assert_eq!(fact.into_parts(), ("e", 1u8, ts, "s", Operation::Retract));
        }
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

            #[test]
            fn keyed_init_with_retirement_split_fold_equals_whole_fold(facts in facts_strategy()) {
                let mut whole: HashMap<String, Keyed> = HashMap::new();
                aggregate_facts_into_with(&mut whole, facts.clone(), |key| Keyed::new(key));
                for (key, agg) in &whole {
                    prop_assert_eq!(&agg.key, key);
                }

                for k in 0..=facts.len() {
                    let (prefix, suffix) = facts.split_at(k);
                    let mut resumed: HashMap<String, Keyed> = HashMap::new();
                    aggregate_facts_into_with(&mut resumed, prefix.to_vec(), |key| Keyed::new(key));
                    for (key, agg) in &resumed {
                        prop_assert_eq!(&agg.key, key, "prefix, split point k = {}", k);
                    }
                    let before = resumed.clone();

                    let touched =
                        aggregate_facts_into_with(&mut resumed, suffix.to_vec(), |key| Keyed::new(key));

                    for (key, agg) in &resumed {
                        prop_assert_eq!(&agg.key, key, "resumed, split point k = {}", k);
                    }
                    prop_assert_eq!(&resumed, &whole, "split point k = {}", k);

                    // Every key whose state changed (inserted, mutated or
                    // retired) is reported as touched.
                    let all_keys: HashSet<&String> = before.keys().chain(resumed.keys()).collect();
                    for key in all_keys {
                        if before.get(key) != resumed.get(key) {
                            prop_assert!(touched.contains(key), "key {} changed but not touched, k = {}", key, k);
                        }
                    }
                    // Touched keys all appear in the delta.
                    for key in &touched {
                        prop_assert!(suffix.iter().any(|f| f.entity() == key));
                    }
                }
            }

            #[test]
            fn open_fold_of_known_facts_equals_plain_fold(facts in facts_strategy()) {
                for k in 0..=facts.len() {
                    let (prefix, suffix) = facts.split_at(k);
                    let mut plain: HashMap<String, Keyed> = HashMap::new();
                    aggregate_facts_into_with(&mut plain, prefix.to_vec(), |key| Keyed::new(key));
                    let mut open = plain.clone();

                    let plain_touched =
                        aggregate_facts_into_with(&mut plain, suffix.to_vec(), |key| Keyed::new(key));
                    let open_suffix: Vec<OpenFact<String, TestValue, String>> = suffix
                        .iter()
                        .cloned()
                        .map(|f| {
                            let (e, v, t, s, op) = f.into_parts();
                            Fact::new(e, Attr::Known(v), t, s, op)
                        })
                        .collect();
                    let open_touched =
                        aggregate_open_facts_into_with(&mut open, open_suffix, |key| Keyed::new(key));

                    prop_assert_eq!(&open, &plain, "split point k = {}", k);
                    prop_assert_eq!(open_touched, plain_touched, "split point k = {}", k);
                }
            }
        }
    }
}
