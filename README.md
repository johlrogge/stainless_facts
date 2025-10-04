# Stainless Facts

A simple, immutable fact stream library inspired by Datomic, designed for systems that need schema evolution and time-travel capabilities.

## What is it?

A fact stream is an append-only log of immutable facts about entities. Each fact represents a single assertion or retraction about an entity's attribute at a specific point in time.

## Core Concepts

### Facts

A fact consists of:
- **Entity** (`E`): What the fact is about
- **Value** (`V`): The attribute and its value
- **Timestamp**: When the fact was recorded
- **Source** (`S`): Who/what created this fact
- **Operation**: `Assert` or `Retract`

### Operations

- **Assert**: Add or update an attribute value
- **Retract**: Remove an attribute value (for multi-valued attributes) or explicitly remove a single-valued attribute

### Aggregation

Facts are aggregated into useful data structures using the `FactAggregator` trait, which handles both known and unknown attributes gracefully.

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

This produces the JSON format the fact stream expects: `{"t": "Bpm", "v": 12800}`

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

assert_fact_value_format!(MyValue::Bpm(12800));  // Validates format
```

## Format

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

## Aggregation Patterns

### Direct Aggregation

Use `aggregate_facts` when your aggregator is the final result:

```rust
#[derive(Default)]
struct Track {
    bpm: Option<u16>,  // Optional fields - no validation
    title: Option<String>,
}

let tracks: HashMap<String, Track> = aggregate_facts(facts);
```

### Builder Pattern with Zero-Copy Aggregation

Use `aggregate_and_build` with the `Buildable` trait for **zero-copy aggregation** with validated results. The builder borrows data during fact processing, and only clones it when producing the final validated output:

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

**Key benefits:**
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

### Handling Unknowns in Aggregation

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
        MusicValue::Bpm(bpm) => self.bpm = Some(*bpm), // Overwrites previous
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
                self.tags.push(tag.clone()); // Accumulates
            }
        }
        // ...
    }
}

fn retract(&mut self, value: &MusicValue, _source: &String) {
    match value {
        MusicValue::Tag(tag) => {
            self.tags.retain(|t| t != tag); // Removes specific value
        }
        // ...
    }
}
```

## Design Principles

1. **Immutable Facts**: Never modify history, only append
2. **Generic Core**: Works with any entity/value/source types
3. **Schema Evolution**: Unknown attributes degrade gracefully
4. **Eventual Consistency**: Aggregates can be rebuilt anytime
5. **Type Safety**: Rust's type system enforces correct handling
6. **Zero-Copy Capable**: Builder pattern enables efficient aggregation

## Benefits

- **Time Travel**: Query historical state by filtering facts
- **Audit Trail**: Complete history of all changes
- **No Merge Conflicts**: Facts are immutable
- **Simple Backup**: Just copy the fact stream file
- **Easy Recovery**: Rebuild aggregates from facts
- **Graceful Evolution**: Add new attributes without breaking old code
- **Performance**: Zero-copy aggregation with builder pattern

## Future Enhancements

- File I/O utilities for reading/writing fact streams
- Iterator adaptors for efficient streaming
- Compression support for large fact streams
- Query DSL for time-travel queries

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.