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

# ── Release ───────────────────────────────────────────────────────────────────
#
# CI runs its own checks directly (see .github/workflows/) rather than through
# just, so there's one place — the workflow file — that defines what actually
# gates a merge or a publish. These recipes are for a human cutting a release:
# bump Cargo.toml and CHANGELOG.md by hand, commit, then `just tag`.

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
