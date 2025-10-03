# Contributing to Fact Stream

Thanks for your interest in contributing! Here's how to get started.

## Development Setup

1. Install Rust (stable): https://rustup.rs/
2. Clone the repository
3. Run tests: `cargo test`

## Before Submitting

Please ensure your changes:

1. **Pass all tests**: `cargo test`
2. **Follow formatting**: `cargo fmt`
3. **Pass clippy lints**: `cargo clippy -- -D warnings`
4. **Build documentation**: `cargo doc --no-deps`

## Pull Request Guidelines

- Keep changes focused and atomic
- Add tests for new functionality
- Update documentation as needed
- Follow existing code style
- Write clear commit messages

## Testing Philosophy

- Tests should have only one reason to fail
- Break unrelated assertions into separate tests
- Use descriptive test names with `rstest` cases

## Code Style

- Use the newtype pattern for all primitives
- Prefer small, incremental steps
- Ask: "Can this one step be done in two steps?"

## Questions?

Open an issue for discussion before starting major work.

## License

By contributing, you agree that your contributions will be licensed under both the MIT License and Apache License 2.0.
