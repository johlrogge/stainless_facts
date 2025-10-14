//! Basic usage of AsyncFactStore
//!
//! Run with: cargo run --example async_basic_usage --features tokio

use chrono::Utc;
use serde::{Deserialize, Serialize};
use stainless_facts::{AsyncFactStore, Fact, Operation};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum MyValue {
    Name(String),
    Count(u32),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== AsyncFactStore Example ===\n");

    // Create a store
    let store: AsyncFactStore<String, MyValue, String> =
        AsyncFactStore::open_or_create("example_async.facts").await?;

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
        Fact::new(
            "item2".to_string(),
            MyValue::Name("Bob".to_string()),
            Utc::now(),
            "user".to_string(),
            Operation::Assert,
        ),
    ];

    println!("Writing {} facts...", facts.len());
    store.append_batch(&facts).await?;
    println!("✓ Facts written\n");

    // Read them back
    println!("Reading all facts:");
    let mut iter = store.iter().await;
    let mut count = 0;
    while let Some(fact) = iter.next().await {
        println!("  {:?}", fact);
        count += 1;
    }
    println!("\n✓ Read {} facts", count);

    Ok(())
}
