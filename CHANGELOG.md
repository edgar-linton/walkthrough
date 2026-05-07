# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
