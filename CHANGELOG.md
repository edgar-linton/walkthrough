# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `AsyncWalkDir::sort_by_metadata` — orders the entries within each directory by a
  key taken from their metadata, e.g. `.sort_by_metadata(|meta| meta.len())`. The
  walker fetches each entry's metadata once before ordering, so the key function
  does no I/O.

  This closes a hole in the async builder: `sort_by` takes a synchronous
  comparator, while metadata is only reachable through `async fn metadata()`, so an
  ordering by size or timestamp could not be expressed at all. Callers were pushed
  into blocking inside the comparator, which panics with "Cannot start a runtime
  from within a runtime" on a multi-threaded runtime and deadlocks on a
  current-thread one. It is also cheaper than a comparator would be: one `stat` per
  entry instead of one per comparison.

  `sort_by` and `sort_by_metadata` are mutually exclusive — the last builder call
  wins — and `group_dir` still applies after either.

## [0.2.0] - 2026-05-07

### Added

- Async support via the `async` feature flag (`tokio` runtime).
  - `AsyncWalkDir` — builder with the same configuration API as `WalkDir`.
  - `AsyncWalker` — stateful walker driven by `.next().await`.
  - `AsyncDirEntry` — type alias for `DirEntry<Async>`; exposes async `metadata()` and `is_hidden()`.
  - Streaming `DirStream` — unsorted traversals hold a live `ReadDir` handle on the stack instead of collecting all entries upfront, keeping memory proportional to tree depth.

### Changed

- `WalkDir` is now a concrete (non-generic) sync builder; the async equivalent is `AsyncWalkDir`.
- `Walker` is now a concrete (non-generic) sync iterator backed by a `DirStream` enum (`Live` / `Sorted` variants).
- Platform-specific `DirEntry::metadata` and `DirEntry::is_hidden` implementations are no longer surfaced as platform-conditional in the public API; both methods are documented once and delegate to private `*_impl` helpers internally.
- Compilation on targets other than Unix and Windows now produces a clear `compile_error!` instead of a cryptic "method not found" cascade.

## [0.1.0] - 2026-05-06

### Added

- Synchronous recursive directory traversal via `WalkDir` / `Walker`.
- `DirEntry` with `path`, `file_name`, `file_type`, `depth`, `metadata`, and `is_hidden`.
- `min_depth` / `max_depth` for depth-bounded traversal.
- `follow_links` with symlink loop detection (device + inode comparison on Unix; volume serial + file index on Windows).
- `skip_hidden` to omit dot-entries (Unix) or `FILE_ATTRIBUTE_HIDDEN` entries (Windows).
- `group_dir` to yield directories before files at each level.
- `sort_by` for a custom comparator applied within each directory.
