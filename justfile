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

# Mirrors [package.metadata.docs.rs] in Cargo.toml — keep the two in step
[group('dev')]
[doc('Build and open the public docs as docs.rs will render them')]
@doc:
    RUSTDOCFLAGS="--cfg docsrs -D warnings" cargo doc --all-features --no-deps --open

# Neither configuration alone checks every link; this is what CI runs
[group('dev')]
[doc('Check the docs build clean with and without the async feature')]
@doc-check:
    cargo doc --no-deps --document-private-items
    cargo doc --no-deps --document-private-items --all-features

# ── Release ───────────────────────────────────────────────────────────────────
#
# CI defines what gates a merge (see .github/workflows/); these are for a human
# cutting a release: bump Cargo.toml and CHANGELOG.md, commit, then `just tag`.

# Dry-run cargo publish without uploading anything
[group('release')]
@publish-dry-run:
    cargo publish --all-features --dry-run

# Tag Cargo.toml's current version and push the tag, triggering the publish workflow
[group('release')]
@tag:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    git tag -a "v$version" -m "v$version"
    git push origin "v$version"
