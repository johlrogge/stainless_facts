//! Demonstrates incremental synchronization pattern
//!
//! Run with: cargo run --example incremental_sync --features io

// Only compile this example with sync IO
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use stainless_facts::{Fact, FactStore, Operation};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum Event {
    Update(String),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store: FactStore<String, Event, String> = FactStore::open_or_create("sync_example.facts")?;

    // Simulate initial sync
    let mut last_sync = DateTime::<Utc>::MIN_UTC;

    println!("=== Initial sync ===");
    for fact in store.iter_from(last_sync) {
        println!("  {:?}", fact);
        last_sync = *fact.timestamp();
    }

    // Add new fact
    thread::sleep(Duration::from_millis(100));
    let new_fact = Fact::new(
        "entity1".to_string(),
        Event::Update("New data".to_string()),
        Utc::now(),
        "system".to_string(),
        Operation::Assert,
    );
    store.append(new_fact)?;

    // Incremental sync - only new facts
    println!("\n=== Incremental sync (only new facts) ===");
    for fact in store.iter_from(last_sync) {
        println!("  {:?}", fact);
    }

    Ok(())
}
