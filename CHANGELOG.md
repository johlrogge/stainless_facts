# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `FactAggregator::apply(&Fact)`, the single hook the folds call per fact. It sees the whole fact (entity, timestamp, source, operation); the default dispatches to `assert`/`retract` (#2)
- `aggregate_facts_into` resumes a fold into an existing map and returns the touched set: every key to which at least one fact was dispatched, a superset of the keys whose map state changed. Keys not in the set are guaranteed unchanged. A retract against an absent entity is a no-op: it is not dispatched and not touched. `retain_touched` sweeps only the touched keys, so cost scales with the delta (#3)
- `aggregate_facts_into_with` builds new entities with `init(&key)` instead of `Default`, and `FactAggregator::is_retired` (default `false`) removes an aggregate after a retract leaves it empty; its key stays in the touched set and a later assert recreates it (#4)
- `Attr<V>` and `OpenFact<E, V, S>` decode an attribute the reader's `V` cannot as `Attr::Unknown(UnknownAttribute)` instead of failing. `aggregate_open_facts_into` and `aggregate_open_facts_into_with` fold them, routing unknown facts to the new `FactAggregator::apply_unknown` (default: `assert_unknown`/`retract_unknown`). `Fact::into_parts` moves a fact's parts out (#6)

### Changed
- **BREAKING**: `aggregate_facts` and `aggregate_and_build` no longer create `Default` entries for entities that only see retracts (#3)
- `UnknownAttribute.v` now defaults to `null` when missing, so unit variants (`{"t":"X"}`) decode; they are re-emitted as `{"t":"X","v":null}` (#6)
- **BREAKING**: File I/O (`FactStore`, `FactStreamReader`/`FactStreamWriter` and their error types) now requires the new `io` feature; `tokio` implies `io`. This reverses the 0.2.0 note that the `io` feature was removed ("Simplified feature model: removed `io` feature").
- The MSRV is now 1.99

### Fixed
- The writer lock is released between writes, so concurrent reads are possible (2aa820a)
- The io stream tests were never compiled; they now run in CI. CI also lints with clippy `--all-targets` and checks the core build on `wasm32-unknown-unknown`

### Internal
- Added `proptest` as a dev-dependency

## [0.2.0] - 2025-10-14

### Added
- `FactStore` for thread-safe, timestamp-ordered fact storage (sync I/O, always included)
- `AsyncFactStore` for async I/O with tokio (optional `tokio` feature)
- `iter_from(timestamp)` for efficient incremental synchronization
- Timestamp ordering validation on append
- Examples: `basic_usage`, `incremental_sync`, and `async_basic_usage`

### Changed
- **BREAKING**: Sync I/O dependencies (`fs2`, `parking_lot`) are now always included
- Simplified feature model: removed `io` feature, only `tokio` feature for async support
- `FactStore` and `AsyncFactStore` can coexist when `tokio` feature is enabled

## [0.1.0] - Initial Release

### Added
- Core fact types and operations
- Fact aggregation with `FactAggregator` trait
- Builder pattern with `Buildable` trait
- Basic I/O with sync and async readers/writers
- Unknown attribute handling
