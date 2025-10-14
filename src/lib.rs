//! # Fact Stream
//!
//! A simple, immutable fact stream library inspired by Datomic, designed for systems
//! that need schema evolution and time-travel capabilities.
//!
//! ## What is a Fact Stream?
//!
//! A fact stream is an append-only log of immutable facts about entities. Each fact
//! represents a single assertion or retraction about an entity's attribute at a
//! specific point in time.
//!
//! ## Serialization Contract
//!
//! **IMPORTANT**: Your value enum must use serde's adjacently tagged representation:
//!
//! ```rust
//! # use serde::{Serialize, Deserialize};
//! #[derive(Serialize, Deserialize)]
//! #[serde(tag = "t", content = "v")]  // Required!
//! enum MyValue {
//!     Bpm(u16),
//!     Title(String),
//! }
//! ```
//!
//! This produces the JSON format the fact stream expects:
//! ```json
//! {"t": "Bpm", "v": 12800}
//! ```
//!
//! Use the [`assert_fact_value_format!`] macro to validate your types at compile time.
//!
//! ## Common Mistakes
//!
//! ❌ **Wrong - will fail validation:**
//! ```should_panic
//! # use serde::{Serialize, Deserialize};
//! # use stainless_facts::assert_fact_value_format;
//! #[derive(Serialize, Deserialize)]
//! enum MyValue {
//!     Bpm(u16),  // Serializes as {"Bpm": 12800} - WRONG!
//! }
//!
//! assert_fact_value_format!(MyValue::Bpm(12800));  // This will panic!
//! ```
//!
//! ✅ **Correct - uses adjacently tagged format:**
//! ```rust
//! # use serde::{Serialize, Deserialize};
//! # use stainless_facts::assert_fact_value_format;
//! #[derive(Serialize, Deserialize)]
//! #[serde(tag = "t", content = "v")]
//! enum MyValue {
//!     Bpm(u16),  // Serializes as {"t": "Bpm", "v": 12800} - CORRECT!
//! }
//!
//! // Validates the format:
//! assert_fact_value_format!(MyValue::Bpm(12800));
//! ```
//!
//! ## Quick Example
//!
//! ```rust
//! use stainless_facts::{Fact, Operation, FactAggregator, aggregate_facts, assert_fact_value_format};
//! use serde::{Serialize, Deserialize};
//! use std::collections::HashMap;
//!
//! // Define your value types with the required format
//! #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
//! #[serde(tag = "t", content = "v")]
//! enum MusicValue {
//!     Bpm(u16),
//!     Title(String),
//!     Tag(String),
//! }
//!
//! // Validate the format
//! assert_fact_value_format!(MusicValue::Bpm(12800));
//! assert_fact_value_format!(MusicValue::Title("Test".to_string()));
//!
//! // Define your aggregation structure
//! #[derive(Default, Debug)]
//! struct Track {
//!     bpm: Option<u16>,
//!     title: Option<String>,
//!     tags: Vec<String>,
//! }
//!
//! // Implement the aggregator
//! impl FactAggregator<String, MusicValue, String> for Track {
//!     fn assert(&mut self, value: &MusicValue, _source: &String) {
//!         match value {
//!             MusicValue::Bpm(bpm) => self.bpm = Some(*bpm),
//!             MusicValue::Title(title) => self.title = Some(title.clone()),
//!             MusicValue::Tag(tag) => {
//!                 if !self.tags.contains(tag) {
//!                     self.tags.push(tag.clone());
//!                 }
//!             }
//!         }
//!     }
//!
//!     fn retract(&mut self, value: &MusicValue, _source: &String) {
//!         match value {
//!             MusicValue::Tag(tag) => self.tags.retain(|t| t != tag),
//!             _ => {}
//!         }
//!     }
//! }
//!
//! // Create and aggregate facts
//! let facts = vec![
//!     Fact::new(
//!         "track1".to_string(),
//!         MusicValue::Bpm(12800),
//!         "2024-01-15T10:00:00Z".parse().unwrap(),
//!         "alice".to_string(),
//!         Operation::Assert
//!     ),
//!     Fact::new(
//!         "track1".to_string(),
//!         MusicValue::Tag("techno".to_string()),
//!         "2024-01-15T10:01:00Z".parse().unwrap(),
//!         "alice".to_string(),
//!         Operation::Assert
//!     ),
//! ];
//!
//! let tracks: HashMap<String, Track> = aggregate_facts(facts);
//! let track = tracks.get("track1").unwrap();
//!
//! assert_eq!(track.bpm, Some(12800));
//! assert_eq!(track.tags, vec!["techno"]);
//! ```
//!
//! ## Serialization Format
//!
//! Facts are stored as newline-delimited JSON arrays:
//!
//! ```json
//! ["track1",{"t":"Bpm","v":12800},"2024-01-15T10:00:00Z","alice","Assert"]
//! ["track1",{"t":"Tag","v":"techno"},"2024-01-15T10:01:00Z","alice","Assert"]
//! ```
//!
//! ## Aggregation Patterns
//!
//! ### Direct Aggregation
//!
//! Use [`aggregate_facts`] when your aggregator is the final result:
//!
//! ```rust
//! # use stainless_facts::{Fact, Operation, FactAggregator, aggregate_facts, assert_fact_value_format};
//! # use serde::{Serialize, Deserialize};
//! # use std::collections::HashMap;
//! #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
//! #[serde(tag = "t", content = "v")]
//! enum MusicValue { Bpm(u16) }
//!
//! // Validate format
//! assert_fact_value_format!(MusicValue::Bpm(12800));
//!
//! #[derive(Default)]
//! struct Track {
//!     bpm: Option<u16>,  // Optional fields - no validation
//! }
//!
//! # impl FactAggregator<String, MusicValue, String> for Track {
//! #     fn assert(&mut self, value: &MusicValue, _source: &String) {
//! #         match value { MusicValue::Bpm(bpm) => self.bpm = Some(*bpm) }
//! #     }
//! #     fn retract(&mut self, _value: &MusicValue, _source: &String) {}
//! # }
//! let tracks: HashMap<String, Track> = aggregate_facts(vec![]);
//! ```
//!
//! ### Builder Pattern with Zero-Copy Aggregation
//!
//! Use [`aggregate_and_build`] with the [`Buildable`] trait for **zero-copy aggregation**
//! with validated results. The builder borrows data during fact processing, and only
//! clones it when producing the final validated output:
//!
//! ```rust
//! # use stainless_facts::{Fact, Operation, FactAggregator, Buildable, aggregate_and_build, assert_fact_value_format};
//! # use serde::{Serialize, Deserialize};
//! # use std::collections::HashMap;
//! # use std::borrow::Cow;
//! #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
//! #[serde(tag = "t", content = "v")]
//! enum MusicValue<'a> {
//!     #[serde(borrow)]
//!     Title(Cow<'a, str>),
//! }
//!
//! // Validate format
//! assert_fact_value_format!(MusicValue::Title(Cow::Borrowed("test")));
//!
//! #[derive(Default)]
//! struct TrackBuilder<'a> {
//!     title: Option<Cow<'a, str>>,  // Borrows during aggregation
//! }
//!
//! struct Track {
//!     title: String,  // Required! Owns after building
//! }
//!
//! # #[derive(Debug)]
//! # enum BuildError { MissingTitle }
//! # impl std::fmt::Display for BuildError {
//! #     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//! #         write!(f, "missing title")
//! #     }
//! # }
//! # impl std::error::Error for BuildError {}
//! # impl<'a> FactAggregator<String, MusicValue<'a>, String> for TrackBuilder<'a> {
//! #     fn assert(&mut self, value: &MusicValue<'a>, _source: &String) {
//! #         match value { MusicValue::Title(title) => self.title = Some(title.clone()) }
//! #     }
//! #     fn retract(&mut self, _value: &MusicValue, _source: &String) {}
//! # }
//! impl<'a> Buildable for TrackBuilder<'a> {
//!     type Output = Track;
//!     type Error = BuildError;
//!     
//!     fn build(self) -> Result<Track, BuildError> {
//!         Ok(Track {
//!             title: self.title.ok_or(BuildError::MissingTitle)?.into_owned(),  // Clone only here
//!         })
//!     }
//! }
//!
//! // Zero-copy during aggregation, validation on build
//! match aggregate_and_build::<_, _, _, TrackBuilder, _>(vec![]) {
//!     Ok(tracks) => println!("Built {} tracks", tracks.len()),
//!     Err(e) => eprintln!("Build failed: {}", e),
//! }
//! ```
//!
//! **Key benefits:**
//! - **Zero allocations during aggregation**: Borrows with `Cow<'a, str>`
//! - **Validation**: Required fields enforced at build time
//! - **Single clone**: Data cloned only once when building final output
//!
//! ## Key Features
//!
//! - **Generic**: Works with any entity, value, and source types
//! - **Schema Evolution**: Gracefully handles unknown attributes
//! - **Time Travel**: Query historical state by filtering facts
//! - **Immutable**: Facts are never modified, only appended
//! - **Type Safe**: Leverages Rust's type system

pub mod io;
pub mod store;
pub use store::{FactStore, StoreError};

// Also re-export IO types for convenience
pub use io::{FactStreamReader, FactStreamWriter, ReadError, WriteError};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::hash::Hash;

/// Operations that can be performed on facts.
///
/// # Examples
///
/// ```
/// use stainless_facts::Operation;
///
/// let assert_op = Operation::Assert;
/// let retract_op = Operation::Retract;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    /// Add or update an attribute value
    Assert,
    /// Remove an attribute value
    Retract,
}

/// Validates at compile-time that a value uses the correct serde format for facts.
///
/// This macro checks that your value serializes with the adjacently-tagged representation
/// required by the fact stream format: `{"t": "VariantName", "v": value}`.
///
/// # Examples
///
/// ```
/// # use stainless_facts::assert_fact_value_format;
/// # use serde::{Serialize, Deserialize};
/// #[derive(Serialize, Deserialize)]
/// #[serde(tag = "t", content = "v")]
/// enum MyValue {
///     Count(u32),
/// }
///
/// // Validates the format at compile time
/// assert_fact_value_format!(MyValue::Count(42));
/// ```
///
/// # What it checks
///
/// - The serialized JSON has a "t" field (the tag)
/// - The serialized JSON has a "v" field (the content)
/// - Both fields are at the top level
///
/// # Panics
///
/// Panics at compile time if the value doesn't serialize with the correct format.
/// The panic message will tell you to add `#[serde(tag = "t", content = "v")]` to your enum.
#[macro_export]
macro_rules! assert_fact_value_format {
    ($value:expr) => {{
        let json = serde_json::to_value(&$value).expect("Failed to serialize value");
        assert!(
            json.get("t").is_some(),
            "Value must serialize with a 't' (tag) field. Add #[serde(tag = \"t\", content = \"v\")] to your enum."
        );
        assert!(
            json.get("v").is_some(),
            "Value must serialize with a 'v' (content) field. Add #[serde(tag = \"t\", content = \"v\")] to your enum."
        );
    }};
}

/// An immutable fact about an entity.
///
/// A fact consists of five components:
/// - **Entity** (`E`): The identifier of what the fact is about
/// - **Value** (`V`): The attribute and its value (must use `#[serde(tag = "t", content = "v")]`)
/// - **Timestamp**: When this fact was recorded
/// - **Source** (`S`): Who or what created this fact
/// - **Operation**: Whether this is an assertion or retraction
///
/// # Type Parameters
///
/// - `E`: Entity type (commonly `String` for IDs)
/// - `V`: Value type (your domain-specific enum with `#[serde(tag = "t", content = "v")]`)
/// - `S`: Source type (commonly `String` for usernames, or `()` if not tracked)
///
/// # Examples
///
/// ```
/// use stainless_facts::{Fact, Operation, assert_fact_value_format};
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// #[serde(tag = "t", content = "v")]
/// enum MyValue {
///     Name(String),
///     Age(u32),
/// }
///
/// // Validate format
/// assert_fact_value_format!(MyValue::Name("Alice".to_string()));
///
/// let fact = Fact::new(
///     "person1".to_string(),
///     MyValue::Name("Alice".to_string()),
///     "2024-01-15T10:00:00Z".parse().unwrap(),
///     "admin".to_string(),
///     Operation::Assert,
/// );
///
/// assert_eq!(fact.entity(), "person1");
/// assert_eq!(fact.operation(), Operation::Assert);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact<E, V, S>(E, V, DateTime<Utc>, S, Operation);

impl<E, V, S> Fact<E, V, S> {
    /// Create a new fact.
    ///
    /// # Examples
    ///
    /// ```
    /// use stainless_facts::{Fact, Operation};
    ///
    /// let fact = Fact::new(
    ///     "entity1",
    ///     42,
    ///     "2024-01-15T10:00:00Z".parse().unwrap(),
    ///     "system",
    ///     Operation::Assert,
    /// );
    /// ```
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
///
/// When deserializing facts, if an attribute type isn't known to your value enum,
/// it can be captured as an `UnknownAttribute` for graceful degradation.
///
/// # Examples
///
/// ```
/// use stainless_facts::{Fact, UnknownAttribute, Operation};
/// use serde_json::Value as JsonValue;
///
/// // Deserialize a fact with an unknown attribute type
/// let json = r#"["entity1",{"t":"NewAttribute","v":42},"2024-01-15T10:00:00Z","alice","Assert"]"#;
/// let fact: Fact<String, UnknownAttribute, String> = serde_json::from_str(json).unwrap();
///
/// assert_eq!(fact.value().t, "NewAttribute");
/// assert_eq!(fact.value().v, JsonValue::Number(42.into()));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownAttribute {
    /// The attribute name as a string
    pub t: String,
    /// The value as a generic JSON value
    pub v: JsonValue,
}

/// Trait for aggregating facts into domain-specific data structures.
///
/// Implement this trait to define how facts are combined into your application's
/// data model. The aggregator receives facts one at a time and updates its state
/// accordingly.
///
/// # Type Parameters
///
/// - `E`: Entity type
/// - `V`: Value type (your domain enum with `#[serde(tag = "t", content = "v")]`)
/// - `S`: Source type
///
/// # Cardinality Patterns
///
/// ## Single-Valued (Latest Wins)
///
/// ```
/// # use stainless_facts::{FactAggregator, assert_fact_value_format};
/// # use serde::{Serialize, Deserialize};
/// #[derive(Clone, Serialize, Deserialize)]
/// #[serde(tag = "t", content = "v")]
/// enum MyValue { Age(u32) }
///
/// // Validate format
/// assert_fact_value_format!(MyValue::Age(42));
///
/// # struct Person { age: Option<u32> }
/// impl FactAggregator<String, MyValue, String> for Person {
///     fn assert(&mut self, value: &MyValue, _source: &String) {
///         match value {
///             MyValue::Age(age) => self.age = Some(*age), // Overwrites
///         }
///     }
///     
///     fn retract(&mut self, value: &MyValue, _source: &String) {
///         match value {
///             MyValue::Age(_) => self.age = None,
///         }
///     }
/// }
/// ```
///
/// ## Multi-Valued (Accumulates)
///
/// ```
/// # use stainless_facts::{FactAggregator, assert_fact_value_format};
/// # use serde::{Serialize, Deserialize};
/// #[derive(Clone, PartialEq, Serialize, Deserialize)]
/// #[serde(tag = "t", content = "v")]
/// enum MyValue { Tag(String) }
///
/// // Validate format
/// assert_fact_value_format!(MyValue::Tag("test".to_string()));
///
/// # struct Item { tags: Vec<String> }
/// impl FactAggregator<String, MyValue, String> for Item {
///     fn assert(&mut self, value: &MyValue, _source: &String) {
///         match value {
///             MyValue::Tag(tag) => {
///                 if !self.tags.contains(tag) {
///                     self.tags.push(tag.clone()); // Accumulates
///                 }
///             }
///         }
///     }
///     
///     fn retract(&mut self, value: &MyValue, _source: &String) {
///         match value {
///             MyValue::Tag(tag) => {
///                 self.tags.retain(|t| t != tag); // Removes specific value
///             }
///         }
///     }
/// }
/// ```
pub trait FactAggregator<E, V, S> {
    /// Handle an assertion fact.
    ///
    /// This method is called when a fact with `Operation::Assert` is encountered.
    /// Implement this to add or update values in your aggregated state.
    fn assert(&mut self, value: &V, source: &S);

    /// Handle a retraction fact.
    ///
    /// This method is called when a fact with `Operation::Retract` is encountered.
    /// Implement this to remove values from your aggregated state.
    fn retract(&mut self, value: &V, source: &S);

    /// Handle an assertion of an unknown attribute.
    ///
    /// This method is called when deserializing encounters an attribute type not
    /// known to your value enum. The default implementation does nothing, but you
    /// can override it to store unknown attributes for later inspection.
    ///
    /// # Examples
    ///
    /// ```
    /// # use stainless_facts::{FactAggregator, assert_fact_value_format};
    /// # use serde::{Serialize, Deserialize};
    /// # use serde_json::Value as JsonValue;
    /// # use std::collections::HashMap;
    /// #[derive(Clone, Serialize, Deserialize)]
    /// #[serde(tag = "t", content = "v")]
    /// enum MyValue { Name(String) }
    ///
    /// // Validate format
    /// assert_fact_value_format!(MyValue::Name("test".to_string()));
    ///
    /// # struct Entity { name: Option<String>, unknowns: HashMap<String, JsonValue> }
    /// impl FactAggregator<String, MyValue, String> for Entity {
    ///     fn assert(&mut self, value: &MyValue, _source: &String) {
    ///         match value {
    ///             MyValue::Name(name) => self.name = Some(name.clone()),
    ///         }
    ///     }
    ///     
    ///     fn retract(&mut self, _value: &MyValue, _source: &String) {}
    ///     
    ///     fn assert_unknown(&mut self, attribute: &str, value: &JsonValue, _source: &String) {
    ///         // Store unknown attributes for later inspection
    ///         self.unknowns.insert(attribute.to_string(), value.clone());
    ///     }
    /// }
    /// ```
    fn assert_unknown(&mut self, _attribute: &str, _value: &JsonValue, _source: &S) {}

    /// Handle a retraction of an unknown attribute.
    ///
    /// The default implementation does nothing.
    fn retract_unknown(&mut self, _attribute: &str, _value: &JsonValue, _source: &S) {}
}

/// Aggregate an iterator of facts into a map of aggregated entities.
///
/// This function processes facts in order, applying each one to the appropriate
/// entity's aggregator. Entities are created on-demand using the `Default` trait.
///
/// # Type Parameters
///
/// - `E`: Entity type (must be hashable and cloneable)
/// - `V`: Value type (must use `#[serde(tag = "t", content = "v")]`)
/// - `S`: Source type
/// - `A`: Aggregator type (must implement `FactAggregator` and `Default`)
/// - `I`: Iterator type yielding facts
///
/// # Examples
///
/// ```
/// use stainless_facts::{Fact, Operation, FactAggregator, aggregate_facts, assert_fact_value_format};
/// use serde::{Serialize, Deserialize};
/// use std::collections::HashMap;
///
/// #[derive(Clone, Serialize, Deserialize)]
/// #[serde(tag = "t", content = "v")]
/// enum Value {
///     Count(u32),
/// }
///
/// // Validate format
/// assert_fact_value_format!(Value::Count(5));
///
/// #[derive(Default)]
/// struct Counter {
///     count: u32,
/// }
///
/// impl FactAggregator<String, Value, ()> for Counter {
///     fn assert(&mut self, value: &Value, _source: &()) {
///         match value {
///             Value::Count(n) => self.count = *n,
///         }
///     }
///     fn retract(&mut self, _value: &Value, _source: &()) {}
/// }
///
/// let facts = vec![
///     Fact::new("item1".to_string(), Value::Count(5),
///               "2024-01-15T10:00:00Z".parse().unwrap(), (), Operation::Assert),
///     Fact::new("item2".to_string(), Value::Count(10),
///               "2024-01-15T10:01:00Z".parse().unwrap(), (), Operation::Assert),
/// ];
///
/// let result: HashMap<String, Counter> = aggregate_facts(facts);
///
/// assert_eq!(result.get("item1").unwrap().count, 5);
/// assert_eq!(result.get("item2").unwrap().count, 10);
/// ```
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

/// ```
/// use stainless_facts::{Fact, Operation, FactAggregator, Buildable, aggregate_and_build, assert_fact_value_format};
/// use serde::{Serialize, Deserialize};
/// use std::borrow::Cow;
///
/// #[derive(Clone, Serialize, Deserialize)]
/// #[serde(tag = "t", content = "v")]
/// enum Value<'a> {
///     #[serde(borrow)]
///     Name(Cow<'a, str>),
/// }
///
/// // Validate format
/// assert_fact_value_format!(Value::Name(Cow::Borrowed("test")));
///
/// #[derive(Default)]
/// struct PersonBuilder<'a> {
///     name: Option<Cow<'a, str>>,  // Borrows during aggregation
/// }
///
/// struct Person {
///     name: String,  // Owns after building
/// }
///
/// #[derive(Debug)]
/// enum BuildError {
///     MissingName,
/// }
///
/// impl std::fmt::Display for BuildError {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         match self {
///             BuildError::MissingName => write!(f, "missing name"),
///         }
///     }
/// }
///
/// impl std::error::Error for BuildError {}
///
/// impl<'a> FactAggregator<String, Value<'a>, ()> for PersonBuilder<'a> {
///     fn assert(&mut self, value: &Value<'a>, _source: &()) {
///         match value {
///             Value::Name(name) => self.name = Some(name.clone()),  // Cheap Cow clone
///         }
///     }
///     fn retract(&mut self, _value: &Value, _source: &()) {}
/// }
///
/// impl<'a> Buildable for PersonBuilder<'a> {
///     type Output = Person;
///     type Error = BuildError;
///     
///     fn build(self) -> Result<Person, BuildError> {
///         Ok(Person {
///             name: self.name.ok_or(BuildError::MissingName)?.into_owned(),  // Clone only here
///         })
///     }
/// }
///
/// let facts = vec![
///     Fact::new("p1".to_string(), Value::Name(Cow::Borrowed("Alice")),
///               "2024-01-15T10:00:00Z".parse().unwrap(), (), Operation::Assert),
/// ];
///
/// match aggregate_and_build::<_, _, _, PersonBuilder, _>(facts) {
///     Ok(people) => {
///         let alice = people.get("p1").unwrap();
///         assert_eq!(alice.name, "Alice");
///     }
///     Err(e) => {
///         eprintln!("Build failed: {}", e);
///     }
/// }
/// ```
pub trait Buildable {
    /// The final output type after validation
    type Output;

    /// Error type when validation fails
    type Error;

    /// Consume the builder and produce the final validated output.
    ///
    /// This method should validate that all required fields are present
    /// and perform any final transformations (like cloning borrowed data).
    fn build(self) -> Result<Self::Output, Self::Error>;
}

/// Error context when aggregation and building fails.
///
/// Provides information about which entity failed to build and at which
/// fact index the failure occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateError<E, BuildError> {
    /// The entity that failed to build
    pub entity: E,

    /// The index of the last fact processed for this entity
    pub last_fact_index: u32,

    /// The underlying builder error
    pub error: BuildError,
}

impl<E, BuildError> std::fmt::Display for AggregateError<E, BuildError>
where
    E: std::fmt::Display,
    BuildError: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Failed to build entity '{}' after processing {} facts: {}",
            self.entity, self.last_fact_index, self.error
        )
    }
}

impl<E, BuildError> std::error::Error for AggregateError<E, BuildError>
where
    E: std::fmt::Debug + std::fmt::Display,
    BuildError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Aggregate facts and build final validated results.
///
/// This function processes facts in order, applying each to the appropriate entity's
/// aggregator (builder). After all facts are processed, it calls `build()` on each
/// aggregator to produce validated final results.
///
/// Unlike [`aggregate_facts`], this function:
/// - Tracks the fact index for each entity
/// - Calls `build()` to validate and produce final output
/// - Returns detailed error information on validation failure
///
/// # Type Parameters
///
/// - `E`: Entity type (must be hashable and cloneable)
/// - `V`: Value type (must use `#[serde(tag = "t", content = "v")]`)
/// - `S`: Source type
/// - `A`: Aggregator/Builder type (must implement both `FactAggregator` and `Buildable`)
/// - `I`: Iterator type yielding facts
///
/// # Errors
///
/// Returns [`AggregateError`] containing:
/// - The entity that failed
/// - The index of the last fact processed for that entity
/// - The builder's validation error
///
/// # Examples
///
/// This example demonstrates zero-copy aggregation: the builder borrows strings
/// during fact processing, and only clones them when building the final output.
///
/// ```
/// use stainless_facts::{Fact, Operation, FactAggregator, Buildable, aggregate_and_build, assert_fact_value_format};
/// use serde::{Serialize, Deserialize};
/// use std::borrow::Cow;
///
/// #[derive(Clone, Serialize, Deserialize)]
/// #[serde(tag = "t", content = "v")]
/// enum Value<'a> {
///     #[serde(borrow)]
///     Name(Cow<'a, str>),
/// }
///
/// // Validate format
/// assert_fact_value_format!(Value::Name(Cow::Borrowed("test")));
///
/// #[derive(Default)]
/// struct PersonBuilder<'a> {
///     name: Option<Cow<'a, str>>,  // Borrows during aggregation
/// }
///
/// struct Person {
///     name: String,  // Owns after building
/// }
///
/// #[derive(Debug)]
/// enum BuildError {
///     MissingName,
/// }
///
/// impl std::fmt::Display for BuildError {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         match self {
///             BuildError::MissingName => write!(f, "missing name"),
///         }
///     }
/// }
///
/// impl std::error::Error for BuildError {}
///
/// impl<'a> FactAggregator<String, Value<'a>, ()> for PersonBuilder<'a> {
///     fn assert(&mut self, value: &Value<'a>, _source: &()) {
///         match value {
///             Value::Name(name) => self.name = Some(name.clone()),  // Cheap Cow clone
///         }
///     }
///     fn retract(&mut self, _value: &Value, _source: &()) {}
/// }
///
/// impl<'a> Buildable for PersonBuilder<'a> {
///     type Output = Person;
///     type Error = BuildError;
///     
///     fn build(self) -> Result<Person, BuildError> {
///         Ok(Person {
///             name: self.name.ok_or(BuildError::MissingName)?.into_owned(),  // Clone only here
///         })
///     }
/// }
///
/// let facts = vec![
///     Fact::new("p1".to_string(), Value::Name(Cow::Borrowed("Alice")),
///               "2024-01-15T10:00:00Z".parse().unwrap(), (), Operation::Assert),
/// ];
///
/// match aggregate_and_build::<_, _, _, PersonBuilder, _>(facts) {
///     Ok(people) => {
///         let alice = people.get("p1").unwrap();
///         assert_eq!(alice.name, "Alice");
///     }
///     Err(e) => {
///         eprintln!("Build failed: {}", e);
///     }
/// }
/// ```
pub fn aggregate_and_build<E, V, S, A, I>(
    facts: I,
) -> Result<HashMap<E, A::Output>, AggregateError<E, A::Error>>
where
    E: Eq + Hash + Clone,
    A: FactAggregator<E, V, S> + Buildable + Default,
    I: IntoIterator<Item = Fact<E, V, S>>,
{
    let mut builders: HashMap<E, A> = HashMap::new();
    let mut fact_counts: HashMap<E, u32> = HashMap::new();

    // Accumulate with index tracking
    for fact in facts {
        let entity = fact.entity().clone();
        let aggregator = builders.entry(entity.clone()).or_default();

        // Track fact index for this entity
        let count = fact_counts.entry(entity).or_insert(0);
        *count += 1;

        match fact.operation() {
            Operation::Assert => aggregator.assert(fact.value(), fact.source()),
            Operation::Retract => aggregator.retract(fact.value(), fact.source()),
        }
    }

    // Build final results with error context
    builders
        .into_iter()
        .map(|(entity, builder)| {
            let last_fact_index = *fact_counts.get(&entity).unwrap_or(&0);

            builder
                .build()
                .map(|output| (entity.clone(), output))
                .map_err(|error| AggregateError {
                    entity,
                    last_fact_index,
                    error,
                })
        })
        .collect()
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

    #[test]
    fn validate_test_value_format() {
        assert_fact_value_format!(TestValue::Bpm(Bpm(12800)));
        assert_fact_value_format!(TestValue::Title("test".to_string()));
        assert_fact_value_format!(TestValue::Tag(Tag("techno".to_string())));
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
        };
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
