//! Basic usage of FactStore
//!
//! Run with: cargo run --example basic_usage --features io

// Only compile this example with sync IO
use chrono::Utc;
use serde::{Deserialize, Serialize};
use stainless_facts::{Fact, FactStore, Operation};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum MyValue {
    Name(String),
    Count(u32),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a store
    let store: FactStore<String, MyValue, String> = FactStore::open_or_create("example.facts")?;

    // Add some facts
    let facts = vec![
        Fact::new(
            "item1".to_string(),
            MyValue::Name("Alice".to_string()),
            Utc::now(),
            "user".to_string(),
            Operation::Assert,
        ),
        Fact::new(
            "item1".to_string(),
            MyValue::Count(42),
            Utc::now(),
            "user".to_string(),
            Operation::Assert,
        ),
    ];

    store.append_batch(&facts)?;

    // Read them back
    println!("All facts:");
    for fact in store.iter() {
        println!("  {:?}", fact);
    }

    Ok(())
}
