# stainless-facts

A simple, immutable fact stream library inspired by Datomic, designed for systems that need schema evolution and time-travel capabilities.

## What is a Fact Stream?

A fact stream is an append-only log of immutable facts about entities. Each fact represents a single assertion or retraction about an entity's attribute at a specific point in time.

A fact consists of:
- **Entity** (`E`): What the fact is about
- **Value** (`V`): The attribute and its value
- **Timestamp**: When the fact was recorded
- **Source** (`S`): Who/what created this fact
- **Operation**: `Assert` or `Retract`
  - **Assert**: Add or update an attribute value
  - **Retract**: Remove an attribute value (for multi-valued attributes) or explicitly remove a single-valued attribute

Facts are aggregated into useful data structures using the `FactAggregator` trait, which handles both known and unknown attributes gracefully.

## Features

- **Core Fact System**: Immutable, timestamped facts with assert/retract operations
- **Aggregation**: Build domain models from fact streams using `FactAggregator`
- **Zero-Copy Capable**: Builder pattern for efficient aggregation with `Buildable` trait
- **I/O Support** (optional): Thread-safe fact storage with `FactStore`
- **Incremental Sync**: Efficiently iterate from any timestamp for distributed systems
- **Async Support** (optional): Tokio-based async readers/writers
- **Schema Evolution**: Graceful handling of unknown attributes
- **Time Travel**: Query historical state by filtering facts
- **Audit Trail**: Complete history of all changes

## Optional Features

By default, stainless-facts provides only the core fact types and aggregation logic with no I/O dependencies.

Enable additional features as needed:

- **`io`**: Enables `FactStore` and synchronous file I/O (adds `fs2` and `parking_lot` dependencies)
- **`tokio`**: Enables async I/O with tokio (implies `io` feature)

```toml
# Cargo.toml

# Core only (no I/O)
stainless-facts = "0.2"

# With synchronous I/O
stainless-facts = { version = "0.2", features = ["io"] }

# With async I/O
stainless-facts = { version = "0.2", features = ["tokio"] }
```

## Installation

```toml
[dependencies]
stainless-facts = "0.2"
```

## Serialization Contract

**IMPORTANT**: Your value enum must use serde's adjacently tagged representation:

```rust
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]  // Required!
enum MyValue {
    Bpm(u16),
    Title(String),
}
```

This produces the JSON format the fact stream expects:
```json
{"t": "Bpm", "v": 12800}
```

Use the `assert_fact_value_format!` macro to validate your types at compile time.

### Common Mistakes

❌ **Wrong - will fail validation:**
```rust
#[derive(Serialize, Deserialize)]
enum MyValue {
    Bpm(u16),  // Serializes as {"Bpm": 12800} - WRONG!
}

assert_fact_value_format!(MyValue::Bpm(12800));  // This will panic!
```

✅ **Correct - uses adjacently tagged format:**
```rust
use stainless_facts::assert_fact_value_format;

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum MyValue {
    Bpm(u16),  // Serializes as {"t": "Bpm", "v": 12800} - CORRECT!
}

// Validates the format:
assert_fact_value_format!(MyValue::Bpm(12800));
```

## Storage Format

Facts are stored as newline-delimited JSON arrays:

```json
["track1",{"t":"Bpm","v":12800},"2024-01-15T10:00:00Z","alice","Assert"]
["track1",{"t":"Tag","v":"techno"},"2024-01-15T10:01:00Z","alice","Assert"]
["track1",{"t":"Tag","v":"minimal"},"2024-01-15T10:02:00Z","alice","Assert"]
["track1",{"t":"Tag","v":"techno"},"2024-01-20T14:00:00Z","alice","Retract"]
```

## Quick Start

```rust
use stainless_facts::{Fact, Operation, FactAggregator, aggregate_facts, assert_fact_value_format};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

// Define your value types with the required format
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum MusicValue {
    Bpm(u16),
    Title(String),
    Tag(String),
}

// Validate the format at compile time
assert_fact_value_format!(MusicValue::Bpm(12800));
assert_fact_value_format!(MusicValue::Title("Test".to_string()));
assert_fact_value_format!(MusicValue::Tag("techno".to_string()));

// Define your aggregation structure
#[derive(Default)]
struct Track {
    bpm: Option<u16>,
    title: Option<String>,
    tags: Vec<String>,
}

// Implement the aggregator
impl FactAggregator<String, MusicValue, String> for Track {
    fn assert(&mut self, value: &MusicValue, _source: &String) {
        match value {
            MusicValue::Bpm(bpm) => self.bpm = Some(*bpm),
            MusicValue::Title(title) => self.title = Some(title.clone()),
            MusicValue::Tag(tag) => {
                if !self.tags.contains(tag) {
                    self.tags.push(tag.clone());
                }
            }
        }
    }

    fn retract(&mut self, value: &MusicValue, _source: &String) {
        match value {
            MusicValue::Tag(tag) => self.tags.retain(|t| t != tag),
            MusicValue::Bpm(_) => self.bpm = None,
            MusicValue::Title(_) => self.title = None,
        }
    }
}

// Use it
fn main() {
    let facts = vec![
        Fact::new(
            "track1".to_string(),
            MusicValue::Bpm(12800),
            "2024-01-15T10:00:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert
        ),
        Fact::new(
            "track1".to_string(),
            MusicValue::Tag("techno".to_string()),
            "2024-01-15T10:01:00Z".parse().unwrap(),
            "alice".to_string(),
            Operation::Assert
        ),
    ];

    let tracks: HashMap<String, Track> = aggregate_facts(facts);
    println!("{:?}", tracks.get("track1"));
    // Some(Track { bpm: Some(12800), title: None, tags: ["techno"] })
}
```

## Using FactStore

When you enable the `io` feature, you can use `FactStore` for persistent, thread-safe fact storage with incremental synchronization:

```rust
use stainless_facts::{Fact, FactStore, Operation};
use serde::{Serialize, Deserialize};
use chrono::Utc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum MyValue {
    Name(String),
    Count(u32),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open or create a store
    let store: FactStore<String, MyValue, String> = 
        FactStore::open_or_create("data.facts")?;
    
    // Append facts (with timestamp ordering validation)
    let fact = Fact::new(
        "item1".to_string(),
        MyValue::Count(42),
        Utc::now(),
        "system".to_string(),
        Operation::Assert,
    );
    store.append(fact)?;
    
    // Incremental sync - only read facts since last sync
    let last_sync = "2024-01-15T00:00:00Z".parse()?;
    for fact in store.iter_from(last_sync) {
        println!("New fact: {:?}", fact);
    }
    
    Ok(())
}
```

### Thread Safety

`FactStore` uses read-write locks for concurrent access:
- Multiple readers can iterate simultaneously
- Writers acquire exclusive locks via file locking
- Timestamp ordering is enforced atomically

### Timestamp Ordering

`FactStore` enforces strict timestamp ordering. Facts with timestamps older than the latest fact in the store will be rejected:

```rust
// This will fail if facts are out of order
store.append_batch(&facts)?;  // Returns StoreError::TimestampOrdering
```

## Aggregation Patterns

### Simple Aggregation

Use `aggregate_facts` when your aggregator is the final result:

```rust
#[derive(Default)]
struct Track {
    bpm: Option<u16>,  // Optional fields - no validation
    title: Option<String>,
}

let tracks: HashMap<String, Track> = aggregate_facts(facts);
```

### Zero-Copy Aggregation with Validation

Use `aggregate_and_build` with the `Buildable` trait for zero-copy aggregation with validated results. The builder borrows data during fact processing, and only clones it when producing the final validated output:

```rust
use stainless_facts::{FactAggregator, Buildable, aggregate_and_build};
use std::borrow::Cow;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum MusicValue<'a> {
    #[serde(borrow)]
    Title(Cow<'a, str>),
}

#[derive(Default)]
struct TrackBuilder<'a> {
    title: Option<Cow<'a, str>>,  // Borrows during aggregation
}

struct Track {
    title: String,  // Required! Owns after building
}

impl<'a> Buildable for TrackBuilder<'a> {
    type Output = Track;
    type Error = BuildError;

    fn build(self) -> Result<Track, BuildError> {
        Ok(Track {
            title: self.title.ok_or(BuildError::MissingTitle)?.into_owned(),  // Clone only here
        })
    }
}

// Zero-copy during aggregation, validation on build
match aggregate_and_build::<_, _, _, TrackBuilder, _>(facts) {
    Ok(tracks) => println!("Built {} tracks", tracks.len()),
    Err(e) => eprintln!("Build failed: {}", e),
}
```

Key benefits:
- **Zero allocations during aggregation**: Borrows with `Cow<'a, str>`
- **Validation**: Required fields enforced at build time
- **Single clone**: Data cloned only once when building final output

## Unknown Attributes

The system handles unknown attributes gracefully using `serde_json::Value`:

```rust
use stainless_facts::UnknownAttribute;
use serde_json::Value as JsonValue;

// When deserializing encounters an unknown attribute type:
let unknown_fact: Fact<String, UnknownAttribute, String> =
    serde_json::from_str(r#"["track1",{"t":"NewAttribute","v":42},"2024-01-15T10:00:00Z","alice","Assert"]"#)
    .unwrap();

// Access the unknown attribute
assert_eq!(unknown_fact.value().t, "NewAttribute");
assert_eq!(unknown_fact.value().v, JsonValue::Number(42.into()));
```

Implement the optional methods to handle unknown attributes:

```rust
impl FactAggregator<String, MusicValue, String> for Track {
    // ... assert/retract implementations ...

    fn assert_unknown(&mut self, attribute: &str, value: &JsonValue, _source: &String) {
        println!("Unknown attribute '{}' with value: {:?}", attribute, value);
        // Optionally store in a HashMap<String, JsonValue> field
    }
}
```

## Cardinality Patterns

### Single-Valued (Latest Wins)

```rust
fn assert(&mut self, value: &MusicValue, _source: &String) {
    match value {
        MusicValue::Bpm(bpm) => self.bpm = Some(*bpm),  // Overwrites previous
        // ...
    }
}
```

### Multi-Valued (Accumulates)

```rust
fn assert(&mut self, value: &MusicValue, _source: &String) {
    match value {
        MusicValue::Tag(tag) => {
            if !self.tags.contains(tag) {
                self.tags.push(tag.clone());  // Accumulates
            }
        }
        // ...
    }
}

fn retract(&mut self, value: &MusicValue, _source: &String) {
    match value {
        MusicValue::Tag(tag) => {
            self.tags.retain(|t| t != tag);  // Removes specific value
        }
        // ...
    }
}
```

## Design Principles

- **Immutable Facts**: Never modify history, only append
- **Generic Core**: Works with any entity/value/source types
- **Schema Evolution**: Unknown attributes degrade gracefully
- **Eventual Consistency**: Aggregates can be rebuilt anytime
- **Type Safety**: Rust's type system enforces correct handling
- **Zero-Copy Capable**: Builder pattern enables efficient aggregation
- **Time Travel**: Query historical state by filtering facts
- **Audit Trail**: Complete history of all changes

## Use Cases

- **No Merge Conflicts**: Facts are immutable
- **Simple Backup**: Just copy the fact stream file
- **Easy Recovery**: Rebuild aggregates from facts
- **Graceful Evolution**: Add new attributes without breaking old code
- **Performance**: Zero-copy aggregation with builder pattern
- **Distributed Systems**: Incremental sync with `FactStore`

## Future Possibilities

- Compression support for large fact streams
- Query DSL for time-travel queries
- Index files for faster timestamp seeking
- Snapshot/restore functionality

## Examples

Check out the examples directory:

```bash
# Basic usage with FactStore
cargo run --example basic_usage --features io

# Incremental synchronization pattern
cargo run --example incremental_sync --features io
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE) or http://opensource.org/licenses/MIT)

at your option.

## Contributing

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
