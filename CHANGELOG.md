# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1] - 2026-11-10

### Documented
- The canonical retraction rule: identity is (entity, value), a Retract cancels the equal live value or is a no-op, and timestamp and source are never part of matching (ADR-001, `docs/adr/001-retraction-identity-and-occurrence-modelling.md`; README "Retraction semantics")
- The `source` parameter of `FactAggregator::retract` and `retract_unknown` is renamed `retracted_by`: the provenance of the retraction, not a match key. Docs only, not breaking (parameter names are not API)

### Fixed
- Examples (README Quick Start, `aggregate_open_facts_into` doctest) cleared a single-valued field on any retract of the attribute; they now clear only when the retracted value equals the current one
- README install snippets said `0.2`; they now say `0.3`

### Internal
- CI uses `actions/checkout@v7`, which runs on Node 24 (v4 targeted the deprecated Node 20 runtime)

## [0.3.0] - 2026-11-03

### Added
- `FactAggregator::apply(&Fact)`, the single hook the folds call per fact. It sees the whole fact (entity, timestamp, source, operation); the default dispatches to `assert`/`retract` (#2)
- `aggregate_facts_into` resumes a fold into an existing map and returns the touched set: every key to which at least one fact was dispatched, a superset of the keys whose map state changed. Keys not in the set are guaranteed unchanged. A retract against an absent entity is a no-op: it is not dispatched and not touched. `retain_touched` sweeps only the touched keys, so cost scales with the delta (#3)
- `aggregate_facts_into_with` builds new entities with `init(&key)` instead of `Default`, and `FactAggregator::is_retired` (default `false`) removes an aggregate after a retract leaves it empty; its key stays in the touched set and a later assert recreates it (#4)
- `Attr<V>` and `OpenFact<E, V, S>` decode an attribute the reader's `V` cannot as `Attr::Unknown(UnknownAttribute)` instead of failing. `aggregate_open_facts_into` and `aggregate_open_facts_into_with` fold them, routing unknown facts to the new `FactAggregator::apply_unknown` (default: `assert_unknown`/`retract_unknown`). `Fact::into_parts` moves a fact's parts out (#6)
- The line codec `Fact::to_line`, `Fact::from_line` and `peek_entity` is public and available in core, without the `io` feature. The stream writers use the same encoder, so their output is `to_line` plus `'\n'` (#7)
- `StreamPosition`, an opaque, serde-serializable byte position in a fact stream, for resumable reads. `FactStreamReader::open_at` and `position`, `read_lines_at`/`read_lines_at_async` (raw lines for relays), `FactStore::read_at` and `AsyncFactStore::read_at`, and `ReadError::InvalidPosition` for positions past the end or not on a line boundary (#5)
- `FactStore::with_lock_timeout`/`lock_timeout` and `DEFAULT_LOCK_TIMEOUT` (5 s), with async equivalents on `AsyncFactStore` (#9)
- `AsyncFactStreamReader`, `AsyncFactStreamWriter` and `AsyncFactStore`'s error type are exported from the crate root (#8)

### Changed
- **BREAKING**: `aggregate_facts` and `aggregate_and_build` no longer create `Default` entries for entities that only see retracts (#3)
- `UnknownAttribute.v` now defaults to `null` when missing, so unit variants (`{"t":"X"}`) decode; they are re-emitted as `{"t":"X","v":null}` (#6)
- **BREAKING**: File I/O (`FactStore`, `FactStreamReader`/`FactStreamWriter` and their error types) now requires the new `io` feature; `tokio` implies `io`. This reverses the 0.2.0 note that the `io` feature was removed ("Simplified feature model: removed `io` feature").
- The MSRV is now 1.99
- **BREAKING**: `FactStreamWriter::write_batch`, `FactStore::append` and `append_batch` (and the async equivalents) return the `StreamPosition` just past the written lines instead of `()` (#5)
- **BREAKING**: Readers take no locks. `ReadError::AlreadyLocked`, `ReadError::LockTimeout` and the reader `open_with_timeout` are removed. Readers yield only complete (newline-terminated) lines (#9)
- **BREAKING**: `AsyncFactStore::latest_timestamp` is now `async` (#9)
- **BREAKING**: `FactStore` and `AsyncFactStore` iterators (`iter`, `iter_from`) yield `Result<Fact, ReadError>` instead of `Fact` (#8)
- **BREAKING**: `StoreError` is one type shared by `FactStore` and `AsyncFactStore` (#8)
- `iter_from(timestamp)` is documented as a time-travel query, not an efficient sync; use `read_at(position)` to sync (#5)
- **BREAKING**: `WriteError`, `ReadError` and `StoreError` are `#[non_exhaustive]`
- **BREAKING**: `append_batch` rejects a batch whose timestamps go backwards within the batch (`TimestampOrdering`). Previously such a batch was written
- Writers repair a torn tail (a file not ending in `'\n'`) by prefixing `'\n'`, so the fragment becomes one malformed line that readers skip
- Writers need read permission on the file
- An empty `append_batch` takes the file lock and returns a line-boundary position
- `read_lines_at` strips `"\r\n"`
- Dropping an async `write_batch`/`append_batch` future no longer cancels a write that has started: the batch lands and the lock is released afterwards
- The store timestamp cache is advanced before writing, so ordering holds even when an append is cancelled

### Fixed
- A long-lived reader no longer blocks writers (#9)
- Concurrent appends to one store no longer race the timestamp-ordering check or fail with `AlreadyLocked`: appends are serialized by a store mutex and wait up to the lock timeout (#9)
- Store iterators no longer silently stop at a malformed line or an I/O error; they yield an `Err` and continue (#8)
- The writer lock is released between writes, so concurrent reads are possible (2aa820a)
- A cancelled async write no longer leaks the exclusive file lock
- Non-contention lock errors (e.g. `ENOLCK`) surface as `Io` instead of a misleading `LockTimeout`
- `open_or_create` no longer fails on a non-UTF-8 line
- The latest-timestamp cache uses the batch max
- The io stream tests were never compiled; they now run in CI. CI also lints with clippy `--all-targets` and checks the core build on `wasm32-unknown-unknown`

### Internal
- Added `proptest` as a dev-dependency

### Migrating from 0.2
- Enable the `io` feature for `FactStore`, `FactStreamReader`/`FactStreamWriter` and their errors (`tokio` implies it).
- Store iterators yield `Result`: in `for fact in store.iter_from(t)` loops, add `let fact = fact?;` and decide how to handle `Err` (it no longer ends iteration silently).
- Retract-only entities no longer appear in `aggregate_facts`/`aggregate_and_build` results; do not rely on a `Default` entry for them.
- Remove matches on `ReadError::AlreadyLocked` and `ReadError::LockTimeout`, and calls to the reader `open_with_timeout`. Writers still report lock errors.
- Add `.await` to `AsyncFactStore::latest_timestamp`.
- Code that names the `()` result of `write_batch`/`append`/`append_batch` must expect `StreamPosition`; `append(f)?;` still compiles.
- For syncing, persist the `StreamPosition` returned by writes (or `FactStreamReader::position`) and resume with `read_at(position)` instead of `iter_from(timestamp)`.
- Match `WriteError`, `ReadError` and `StoreError` with a wildcard arm; they are `#[non_exhaustive]`.

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
