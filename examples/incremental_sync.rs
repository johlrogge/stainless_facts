//! Demonstrates incremental synchronization with stream positions
//!
//! Run with: cargo run --example incremental_sync --features io

use chrono::Utc;
use serde::{Deserialize, Serialize};
use stainless_facts::{Fact, FactStore, Operation, StreamPosition};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum Event {
    Update(String),
}

fn update(entity: &str, text: &str) -> Fact<String, Event, String> {
    Fact::new(
        entity.to_string(),
        Event::Update(text.to_string()),
        Utc::now(),
        "system".to_string(),
        Operation::Assert,
    )
}

/// Read every fact after `from`, and return the position to resume from
/// next time.
fn sync(
    store: &FactStore<String, Event, String>,
    from: StreamPosition,
) -> Result<StreamPosition, Box<dyn std::error::Error>> {
    let mut reader = store.read_at(from)?;
    for fact in reader.by_ref() {
        println!("  {:?}", fact?);
    }
    Ok(reader.position())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join("stainless_facts_sync_example.facts");
    let _ = std::fs::remove_file(&path);
    let store: FactStore<String, Event, String> = FactStore::open_or_create(&path)?;

    store.append_batch(&[update("entity1", "first"), update("entity2", "first")])?;

    // Initial sync: read everything. A real consumer would persist the
    // returned position (it serializes as a plain integer).
    println!("=== Initial sync ===");
    let position = sync(&store, StreamPosition::START)?;

    // More facts arrive, two of them in the same batch (and so with the
    // same timestamp, which a timestamp cursor could not tell apart).
    store.append_batch(&[update("entity1", "second"), update("entity2", "second")])?;

    // Incremental sync: only the new facts, each exactly once.
    println!("\n=== Incremental sync (only new facts) ===");
    let position = sync(&store, position)?;

    // Nothing new: nothing is read, and the position stays put.
    println!("\n=== Incremental sync (nothing new) ===");
    let unchanged = sync(&store, position)?;
    assert_eq!(unchanged, position);

    std::fs::remove_file(&path)?;
    Ok(())
}
