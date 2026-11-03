# Default: run all checks
default: check-all test-all clippy-all

# List all available commands
list:
    just --list

# watch and run all checks when files change
[group('build')]
watch:
    cargo watch -s 'just check-all test-all clippy-all'

# === Check Commands ===

# check all feature combinations
[group('build')]
check-all: check-core check-io check-tokio

[group('build')]
check-core:
    @echo "=== Checking: core (no features) ==="
    cargo check

[group('build')]
check-io:
    @echo "=== Checking: io (sync I/O) ==="
    cargo check --features io

[group('build')]
check-tokio:
    @echo "=== Checking: tokio (sync + async I/O) ==="
    cargo check --features tokio

# check the core build for wasm32 (requires the wasm32-unknown-unknown target)
[group('build')]
check-wasm:
    @echo "=== Checking: core for wasm32-unknown-unknown ==="
    cargo check --target wasm32-unknown-unknown

# === Test Commands ===

# test all feature combinations
[group('build')]
test-all: test-core test-io test-tokio

[group('build')]
test-core:
    @echo "=== Testing: core (no features) ==="
    cargo test

[group('build')]
test-io:
    @echo "=== Testing: io (sync I/O) ==="
    cargo test --features io

[group('build')]
test-tokio:
    @echo "=== Testing: tokio (sync + async I/O) ==="
    cargo test --features tokio

# === Clippy Commands ===

# clippy all feature combinations
[group('build')]
clippy-all: clippy-core clippy-io clippy-tokio

[group('build')]
clippy-core:
    @echo "=== Clippy: core (no features) ==="
    cargo clippy --all-targets -- -D warnings

[group('build')]
clippy-io:
    @echo "=== Clippy: io (sync I/O) ==="
    cargo clippy --all-targets --features io -- -D warnings

[group('build')]
clippy-tokio:
    @echo "=== Clippy: tokio (sync + async I/O) ==="
    cargo clippy --all-targets --features tokio -- -D warnings

# === Build Commands ===

# build all feature combinations
[group('build')]
build-all: build-core build-io build-tokio

[group('build')]
build-core:
    @echo "=== Building: core (no features) ==="
    cargo build

[group('build')]
build-io:
    @echo "=== Building: io (sync I/O) ==="
    cargo build --features io

[group('build')]
build-tokio:
    @echo "=== Building: tokio (sync + async I/O) ==="
    cargo build --features tokio

# just build core
[group('build')]
build: build-core

# === CI Command ===

# run all checks (same as default target)
[group('build')]
ci: check-all test-all clippy-all

# === Quick Commands ===

# quick check core
[group('build')]
quick:
    cargo check

# quick test core
[group('build')]
quick-test:
    cargo test

# === Formatting ===

# format code
[group('build')]
fmt:
    cargo fmt

# check formatting
[group('build')]
fmt-check:
    cargo fmt -- --check

# === Documentation ===

# generate and open documentation
[group('docs')]
doc:
    cargo doc --all-features --no-deps --open

# build documentation without opening it
[group('docs')]
doc-check:
    cargo doc --all-features --no-deps
