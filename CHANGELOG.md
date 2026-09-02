# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-09-02

### Added

- `AsyncWalkDir::concurrency` — how many entries of one directory may have I/O
  in flight at once, defaulting to 32:

  ```rust
  let walker = AsyncWalkDir::new("/mnt/share")
      .concurrency(256)
      .sort_by(|entry| async move { entry.metadata().await.map(|m| m.len()).unwrap_or(0) })
      .walker()
      .await;
  ```

  Every per-entry resolve the walker performs is bounded by it: the `sort_by`
  key, the file type on a filesystem that does not populate `d_type`, and the
  hidden flag on Windows. Previously each was awaited one entry at a time, so a
  directory's latency was the *sum* of its entries' rather than the longest of
  them — and no caller could fix that from outside, because the work happens
  inside the walker.

  The size of the win is the per-entry latency, which is why it is a setting
  rather than a constant. A local filesystem resolves metadata in microseconds,
  and 2200 entries ordered by size went from 38.7 ms to 15.2 ms — bounded from
  there by Tokio's blocking-pool dispatch, not by the syscall. A network mount
  or FUSE layer where one `stat` costs milliseconds is bound by round trips
  instead: 2000 entries at 5 ms each take 10.2 s sequentially, 649 ms at a
  concurrency of 16, and 175 ms at 64. That case — a directory listing served
  from a request handler over a network mount — is what motivated the change.

  Concurrency never affects the order entries are yielded in: results are placed
  by directory position, not by completion. There is deliberately no unbounded
  setting, since a directory with a million children would then start a million
  concurrent resolves. The synchronous walker has no counterpart, having nothing
  to overlap.

- `AsyncWalkDir::limit` and `AsyncWalkDir::offset` — together
  `skip(offset).take(limit)` over the walk the same builder would otherwise
  produce, so a listing endpoint can expose the same `LIMIT`/`OFFSET` pair as
  the rest of an SQL-backed API:

  ```rust
  let walker = AsyncWalkDir::new(root).offset(200).limit(100).walker().await;
  ```

  Counted over yielded entries across the whole traversal rather than per
  directory, so they compose with `min_depth` and `skip_hidden` the way `LIMIT`
  composes with a `WHERE` clause; errors the walk reports count as entries. An
  offset past the end of the walk yields nothing and is not an error, and a
  skipped directory is still descended into — a page is a window over the
  traversal, not a truncation of it.

  Two properties worth passing on to whoever consumes the pages. Entries created
  or removed between two requests can make a page skip or repeat an entry, as
  they can with an SQL offset; a position is not a snapshot. And an offset is
  only meaningful over a reproducible order, so set `sort_by` or `group_dir`
  when paginating — filesystem order is not reproducible across two `read_dir`
  calls of the same directory.

  `limit` bounds what a caller receives, not what the filesystem is asked:
  ordering by a key still requires the key of every candidate in every directory
  the walk opens. `concurrency` is what makes that affordable.

### Changed

- **Breaking (behaviour):** entries whose sort keys compare equal are now
  ordered by file name. The order within a directory is, in precedence order,
  `group_dir` if set, then entries that failed outright, then the key, then the
  file name.

  This makes the order total, and therefore reproducible between two walks of
  the same directory — the previous behaviour left equal-keyed entries in
  filesystem order, which is not reproducible across re-opens, so a directory of
  equal-sized files could come back differently ordered on two requests for the
  same page. `offset` has nothing to stand on without it.

  `group_dir` is now part of that comparison rather than a sort of the
  already-sorted result. The outcome is the same, and it is one sort instead of
  two.

### Fixed

- A directory's metadata is no longer resolved when its entry is constructed.
  `d_type` already gives the file type and `d_ino` the directory's own inode, so
  the resolve recomputed two values the walker had and cost one `stat` per
  subdirectory — including subdirectories the walk never descends into. Listing
  2200 subdirectories with `max_depth(1)` went from 32.5 ms to 4.7 ms, 14.8 µs
  to 2.1 µs per entry, bringing a subdirectory to roughly the cost of a plain
  file. A recursive walk performs the same number of resolves as before, just at
  the point of descent: `ancestor` resolves on demand through the entry's
  metadata cache.

- On Unix, ancestor bookkeeping is skipped when `follow_links` is off, removing
  another `stat` per descended directory. A symlink loop is only reachable when
  links are followed there: a symlink is otherwise never descended into, and the
  platform does not permit a hardlinked directory, so the check could not fire.
  Windows keeps the bookkeeping either way, since a directory junction is a
  reparse point the platform can report as a directory rather than as a link, so
  a cycle through one is not provably unreachable.

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
