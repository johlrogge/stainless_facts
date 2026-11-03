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
- **I/O Support** (optional, `io` feature): Thread-safe fact storage with `FactStore`
- **Incremental Sync**: Efficiently iterate from any timestamp for distributed systems
- **Async Support** (optional, `tokio` feature): Tokio-based async readers/writers
- **Schema Evolution**: Graceful handling of unknown attributes
- **Time Travel**: Query historical state by filtering facts
- **Audit Trail**: Complete history of all changes

## Optional Features

By default (`default = []`), stainless-facts provides only the core fact types and aggregation logic. The core depends only on `serde`, `serde_json`, `chrono` and `thiserror`, and builds for `wasm32-unknown-unknown`.

Enable additional features as needed:

- **`io`**: Enables `FactStore`, `FactStreamReader`/`FactStreamWriter` (and their error types) and synchronous file I/O (adds `fs2` and `parking_lot` dependencies)
- **`tokio`**: Enables async I/O with tokio (implies `io`)

```toml
# Cargo.toml

# Core only (no file I/O)
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

# To use FactStore or the stream reader/writer:
# stainless-facts = { version = "0.2", features = ["io"] }
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

`FactStore` requires the `io` feature (`stainless-facts = { version = "0.2", features = ["io"] }`), as do the examples in this section and the sync and thread-safety sections below; only the line codec works without it. It provides persistent, thread-safe fact storage with incremental synchronization:

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
    
    // Time-travel query - facts at or after a timestamp.
    // Items are `Result<Fact, ReadError>`: a malformed line or I/O error is
    // yielded as an `Err`, not silently treated as the end of the stream.
    let since = "2024-01-15T00:00:00Z".parse()?;
    for fact in store.iter_from(since) {
        let fact = fact?;
        println!("Fact: {:?}", fact);
    }
    
    Ok(())
}
```

`iter_from` scans the file from the start and compares timestamps; it is not a sync mechanism. To sync, use `StreamPosition` (below).

### Syncing with StreamPosition (`io`)

Timestamps are not unique (a batch shares one), so they cannot mark where you stopped. A `StreamPosition` is an opaque byte position of the next unread line. `append`, `append_batch` and `FactStreamWriter::write_batch` return the position just past what they wrote; it is `Serialize`/`Deserialize`, so you can persist it:

```rust
use stainless_facts::StreamPosition;

// Writing returns the position after the written lines
let position: StreamPosition = store.append(fact)?;
let saved = serde_json::to_string(&position)?;   // persist wherever you like

// Later: resume exactly after that position
let position: StreamPosition = serde_json::from_str(&saved)?;
let mut reader = store.read_at(position)?;
for fact in &mut reader {
    let fact = fact?;
    // apply fact...
}
let saved = reader.position();                   // position after the last complete line
```

`StreamPosition::START` reads from the beginning. A position past the end of the file, or not on a line boundary, fails with `ReadError::InvalidPosition`. `AsyncFactStore::read_at` is the async twin (`tokio` feature).

Relays that forward lines without decoding them can use `read_lines_at(path, position, limit)` (`read_lines_at_async` with `tokio`), which returns up to `limit` raw lines and the position for the next page:

```rust
let (lines, next) = stainless_facts::read_lines_at("data.facts", position, 500)?;
```

### Line codec

The storage format is owned by the crate. `Fact::to_line`, `Fact::from_line` and `peek_entity` are available in core (no `io` feature) and match what the stream writers produce:

```rust
let line: String = fact.to_line()?;                       // one line, no trailing '\n'
let fact: Fact<String, MyValue, String> = Fact::from_line(&line)?;
let entity: String = stainless_facts::peek_entity(&line)?; // decodes only the entity
```

`peek_entity` works without knowing the line's value type, for example to route lines by entity.

### Thread Safety

Reads never block writes:
- Readers take no locks and yield only complete (newline-terminated) lines, so a line still being appended is left for the next read
- Writers take an exclusive file lock for the duration of each write and release it afterwards
- `FactStore` also holds a mutex around appends, so the timestamp-ordering check and the write are atomic across threads
- A write waits up to 5 s (`DEFAULT_LOCK_TIMEOUT`) for the lock before failing; change it with `FactStore::with_lock_timeout(Duration)` and read it with `lock_timeout()`
- Windows caveat: file locks there are mandatory (`LockFileEx`), so a read that overlaps a write can block or fail; on Unix they are advisory and readers are never blocked

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

### Seeing the Whole Fact with `apply`

`FactAggregator::apply(&Fact)` is the one hook every fold calls per fact. The default dispatches to `assert`/`retract`; override it when you need the timestamp or entity (`assert` and `retract` stay required):

```rust
fn apply(&mut self, fact: &Fact<String, MusicValue, String>) {
    if let MusicValue::Bpm(bpm) = fact.value() {
        if fact.operation() == Operation::Assert {
            self.bpm = Some((*bpm, *fact.timestamp()));
        }
    }
}
```

### Incremental Folding

`aggregate_facts_into` folds new facts into an existing map and returns the touched set: every key at least one fact was dispatched to, a superset of the keys whose state changed. Keys not in the set are guaranteed unchanged. A retract against an unknown entity is a no-op (no entry, not touched). Folding a suffix into the fold of a prefix gives the same map as folding everything. `retain_touched` then sweeps only those keys:

```rust
use stainless_facts::{aggregate_facts_into, retain_touched};

let mut tracks: HashMap<String, Track> = HashMap::new();
let touched = aggregate_facts_into(&mut tracks, new_facts);

// Drop touched tracks that ended up without a title
retain_touched(&mut tracks, &touched, |_key, track| track.title.is_some());
```

Override `FactAggregator::is_retired` to remove an aggregate automatically when a retract leaves it empty. Its key stays in the touched set, and a later assert recreates it.

### Keyed Init

`aggregate_facts_into_with` builds new entities from their key instead of `Default`, so `A` needs no `Default` impl:

```rust
let touched = aggregate_facts_into_with(&mut tracks, new_facts, |id: &String| Track::new(id));
```

`init` runs only when an assert reaches an entity not in the map.

## Unknown Attributes (Schema Evolution)

A reader whose value enum predates a newer writer would fail to deserialize the new attributes. Read the stream as `OpenFact<E, V, S>` instead: an attribute `V` cannot decode becomes `Attr::Unknown(UnknownAttribute { t, v })` (a missing `v` is `null`, and is re-emitted as `"v":null`) rather than an error. This includes a known tag whose payload changed type; deserialize plain `Fact` if you want that to be an error. Anything that is not an object with a string `t` and no keys other than `t` and `v` is still an error.

Fold open facts with `aggregate_open_facts_into` (or `aggregate_open_facts_into_with` for keyed init). Known facts go to `apply`; unknown ones go to `apply_unknown`, which by default calls `assert_unknown`/`retract_unknown`:

```rust
use stainless_facts::{aggregate_open_facts_into, FactAggregator, OpenFact};
use serde_json::Value as JsonValue;

impl FactAggregator<String, MusicValue, String> for Track {
    // ... assert/retract implementations ...

    fn assert_unknown(&mut self, attribute: &str, value: &JsonValue, _source: &String) {
        self.unknown.push((attribute.to_owned(), value.clone()));
    }
}

let line = r#"["track1",{"t":"Rating","v":5},"2024-01-15T10:00:00Z","alice","Assert"]"#;
let fact: OpenFact<String, MusicValue, String> = serde_json::from_str(line)?;

let mut tracks: HashMap<String, Track> = HashMap::new();
aggregate_open_facts_into(&mut tracks, vec![fact]);
```

Override `apply_unknown(&Fact<E, UnknownAttribute, S>)` if you need the timestamp or entity of an unknown fact. Unknown facts follow the same entity, touched-set and `is_retired` rules as known ones. Parsing lines needs only `serde_json`; reading them from a file via `FactStreamReader` requires the `io` feature.

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
