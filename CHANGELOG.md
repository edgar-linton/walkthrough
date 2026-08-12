# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-08-12

### Changed

- **Breaking:** `AsyncWalkDir::sort_by` now takes a key function computed
  asynchronously, `Fn(Arc<AsyncDirEntry>) -> impl Future<Output: Ord>`, instead of a
  synchronous comparator:

  ```rust
  // before — could not order by anything that needs I/O
  .sort_by(|a, b| a.file_name().cmp(b.file_name()))
  // after
  .sort_by(|entry| async move { entry.file_name().to_owned() })
  ```

  This closes a hole in the async builder: metadata is only reachable through
  `async fn metadata()`, so an ordering by size or timestamp could not be expressed
  at all. Callers were pushed into blocking inside the comparator, which panics with
  "Cannot start a runtime from within a runtime" on either runtime flavour — a `500`
  when the walk runs inside a request handler.

  The key is awaited once per entry, before ordering, so an ordering by size costs
  one `stat` per entry rather than one per comparison, and a key that awaits nothing
  costs nothing. Compound orderings that mix awaited and synchronous properties are
  tuple keys; descending order is `std::cmp::Reverse`. The entry is handed to the key
  as an `Arc` so it can be held across an `await`, and metadata the key resolves
  stays cached on the entry the walk yields. Entries that failed outright are placed
  ahead of the rest without a key being computed for them, `group_dir` is still
  applied afterwards, and a second `sort_by` call replaces the first.

  `WalkDir::sort_by` is unchanged: a synchronous comparator is the right shape there,
  since `DirEntry::metadata` is directly readable and cached.

### Fixed

- `DirEntry` is now `Clone` in practice. The derived implementation demanded
  `T: Clone` of the state marker, which neither `Sync` nor `Async` implements, so it
  applied to no entry at all.

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
