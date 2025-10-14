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
check-all: check-default check-tokio

[group('build')]
check-default:
    @echo "=== Checking: Default (sync I/O) ==="
    cargo check

[group('build')]
check-tokio:
    @echo "=== Checking: tokio (sync + async I/O) ==="
    cargo check --features tokio

# === Test Commands ===

# test all feature combinations
[group('build')]
test-all: test-default test-tokio

[group('build')]
test-default:
    @echo "=== Testing: Default (sync I/O) ==="
    cargo test

[group('build')]
test-tokio:
    @echo "=== Testing: tokio (sync + async I/O) ==="
    cargo test --features tokio

# === Clippy Commands ===

# clippy all feature combinations
[group('build')]
clippy-all: clippy-default clippy-tokio

[group('build')]
clippy-default:
    @echo "=== Clippy: Default (sync I/O) ==="
    cargo clippy -- -D warnings

[group('build')]
clippy-tokio:
    @echo "=== Clippy: tokio (sync + async I/O) ==="
    cargo clippy --features tokio -- -D warnings

# === Build Commands ===

# build all feature combinations
[group('build')]
build-all: build-default build-tokio

[group('build')]
build-default:
    @echo "=== Building: Default (sync I/O) ==="
    cargo build

[group('build')]
build-tokio:
    @echo "=== Building: tokio (sync + async I/O) ==="
    cargo build --features tokio

# just build default
[group('build')]
build: build-default

# === CI Command ===

# run all checks (same as default target)
[group('build')]
ci: check-all test-all clippy-all

# === Quick Commands ===

# quick check default
[group('build')]
quick:
    cargo check

# quick test default
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

# === Cleanup ===

# clean build artifacts
[group('build')]
clean:
    cargo clean
