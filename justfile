# List available recipes
default:
    @just --list

# ── Development ───────────────────────────────────────────────────────────────

# Format code in place
[group('dev')]
@fmt:
    cargo fmt --all

# Lint (warnings are advisory)
[group('dev')]
@clippy:
    cargo clippy --all-targets --all-features
alias lint := clippy

# Quick compile check without producing artifacts
[group('dev')]
@check:
    cargo check --all-targets --all-features

# Unit tests
[group('dev')]
@test-unit:
    cargo test --lib --bins

# Documentation tests
[group('dev')]
@test-doc:
    cargo test --doc

# Integration tests
[group('dev')]
@test-integration:
    cargo test --tests

# Run all tests
[group('dev')]
test: test-unit test-doc test-integration

# Format, lint, and run all tests — run before committing
[group('dev')]
pre-commit: fmt clippy test

# Generate and open an HTML coverage report (requires cargo-llvm-cov)
[group('dev')]
@cov:
    cargo llvm-cov --all-features --workspace --open

# ── CI ────────────────────────────────────────────────────────────────────────

# Verify formatting without modifying files
[group('ci')]
@ci-fmt:
    cargo fmt --all -- --check

# Lint, treating warnings as errors
[group('ci')]
@ci-clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Build documentation
[group('ci')]
@ci-doc:
    cargo doc --no-deps --document-private-items

# Generate LCOV coverage report for upload
[group('ci')]
@ci-coverage:
    cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info

# Full CI suite (mirrors GitHub Actions)
[group('ci')]
ci: ci-fmt ci-clippy ci-doc test
