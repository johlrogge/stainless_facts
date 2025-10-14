# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
