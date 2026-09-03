# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Internal

- Module layout, with no change to the public API. `iter.rs` is split into
  `options.rs`, which holds only the `WalkDirOptions` both walkers share, and
  `sync/iter.rs`, which holds the `WalkDir` builder and its comparator; `WalkDir`
  keeps its re-export at the crate root. The 931-line `async/state.rs` is split
  into `async/iter.rs` (the `AsyncWalkDir` builder and the closure types it
  constructs), `async/concurrent.rs` (the bounded fan-out: `map_bounded`,
  `resolve`, `sort_by_key`) and `async/state.rs` (the walker), so the async half
  now mirrors the sync split of builder from walker.

### Changed

- `follow_links` now resolves a symlink to a *file* on Windows, not just a
  symlink to a directory. Windows's four constructors gated on
  `is_symlink_dir()`, so on Windows a followed file symlink kept
  `file_type().is_symlink()` where Unix reported a file, and a dangling one was
  yielded as a valid entry instead of producing an I/O error. All eight
  constructors now resolve on `is_symlink() && follow_link` alone, so
  `follow_links` means the same thing on both platforms.

- On Windows, `metadata()` no longer re-reads from the filesystem when
  `follow_links` is set. The constructors now resolve a followed symlink, so
  the stored value is already the one to report; the re-read only ever existed
  to cover the `is_symlink_dir()` gap above. It reflects the state the walk
  saw, which is what Unix has always returned.

### Fixed

- The `std::os::unix::fs::DirEntryExt` impl for `DirEntry<Async>` was missing
  its `doc(cfg)` annotation, so docs.rs rendered it without the "Available on
  Unix" badge.

### Documentation

- The `async` module now says that the feature means tokio specifically, and
  when to prefer `WalkDir` inside `tokio::task::spawn_blocking` instead: it
  keeps a request handler off its worker thread just as well, with one handover
  to the blocking pool rather than thousands.

### Performance

- Windows no longer opens a handle per directory just to detect loops. Identity
  there means `CreateFile` + `GetFileInformationByHandle`, the most expensive
  per-directory operation on the platform, and both walkers ran it for every
  directory they descended into even with `follow_links` off — where Unix skips
  it entirely, since without following a link no cycle is reachable. Windows
  cannot make that assumption, because a directory junction can be reported as
  a plain directory rather than a link, but it can make the narrower one that a
  cycle needs a reparse point somewhere on the path: NTFS allows hard links to
  files only. The directory scan's own record carries
  `FILE_ATTRIBUTE_REPARSE_POINT`, so identity now waits for the first reparse
  point on the current path and then fills in the levels above it in one pass.
  A tree holding no reparse point at all, which is the overwhelming majority,
  opens no handle.

- The synchronous walker now skips ancestor bookkeeping on Unix when
  `follow_links` is off, as the asynchronous one already did — without
  following a link no cycle is reachable, so the `stat` per directory that
  identity costs buys nothing. Measured on a 42 000-entry tree: 2001 `stat`
  calls down to 1, and 18 ms down to 16 ms with a warm page cache. On a network
  filesystem, where each round trip costs a fraction of a millisecond rather
  than a microsecond, the same 2000 calls are worth roughly a second.

- Dropped a redundant `metadata` call from six of the eight `DirEntry`
  constructors. For anything but a followed symlink, the metadata already in
  hand — `symlink_metadata` for a root, the directory scan's own record for a
  child — describes the same file, so resolving it again was a wasted syscall
  per traversal root and, on Windows, per directory child.

## [0.4.0] - 2026-09-02

### Added

- `AsyncWalkDir::filter_entry` — an async predicate that decides each entry's
  fate before the walker descends, returning the new `Filtering` enum:

  ```rust
  use walkthrough::{AsyncWalkDir, Filtering};

  let walker = AsyncWalkDir::new(".")
      .filter_entry(|entry| async move {
          if entry.file_name() == "target" {
              Filtering::IgnoreDir
          } else {
              Filtering::Continue
          }
      })
      .walker();
  ```

  It is applied where `skip_hidden` is, so `IgnoreDir` prunes a subtree without
  reading it — the one thing a downstream combinator cannot do, since by the
  time a stream sees a directory the walker has already opened it.
  `IgnoreEntry` drops the entry but still descends. The predicate may await
  `metadata` and is bounded by `concurrency`; the traversal root is exempt, and
  an entry that failed to resolve is reported without consulting it.

- `AsyncWalker` implements `futures_core::Stream`, so the combinators of
  `futures::StreamExt` and `tokio_stream::StreamExt` — `skip`, `take`,
  `filter`, `map` — apply to a walker directly.

- `AsyncWalkDir::concurrency` — how many entries of one directory may have I/O
  in flight at once, defaulting to 32:

  ```rust
  let walker = AsyncWalkDir::new("/mnt/share")
      .concurrency(256)
      .sort_by(|entry| async move { entry.metadata().await.map(|m| m.len()).unwrap_or(0) })
      .walker();
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
  let walker = AsyncWalkDir::new(root).offset(200).limit(100).walker();
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

- **Breaking (API):** the synchronous state marker is renamed from `Sync` to
  `Blocking`, so `DirEntry<Sync>` becomes `DirEntry<Blocking>`. The old name
  shadowed `std::marker::Sync` for anyone writing `use walkthrough::*`, which
  broke every `T: Send + Sync` bound in that module.

- **Breaking (API):** `AsyncWalkDir::walker` is no longer `async`. The root is
  resolved on the first entry instead of at construction, so a builder no
  longer needs a runtime to produce a walker, and an unreadable root arrives as
  the first `Err` rather than failing at `walker()`.

- **Breaking (API):** `AsyncWalker::next` is renamed to `next_entry`, as
  tokio's `ReadDir::next_entry` is, so it no longer shadows `StreamExt::next`
  for a caller who has that trait in scope.

- **Breaking (API):** `AsyncWalker::into_stream` is removed. `AsyncWalker` now
  implements `Stream` itself, so the conversion had nothing left to do.

- **Breaking (API):** the asynchronous module is renamed from `r#async` to
  `aio`. `Async`, `AsyncDirEntry`, `AsyncWalkDir`, `AsyncWalker` and
  `Filtering` are re-exported at the crate root as well, so either path names
  them:

  ```rust
  use walkthrough::AsyncWalkDir;
  // or
  use walkthrough::aio::AsyncWalkDir;
  ```

  `async` is a keyword, so the old module could only be named as the raw
  identifier `walkthrough::r#async`, which every import in a consumer paid for.
  The root is where 0.3.0 exposed these names too — `Filtering` excepted, as it
  is new here — and it puts the two state markers on one path, so generic
  `DirEntry<T>` code no longer reaches `Blocking` and `Async` differently.

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

- The async integration tests are declared as `[[test]]` targets carrying
  `required-features = ["async"]` instead of opening with an inner
  `#![cfg(feature = "async")]`. Without the feature cargo now skips those five
  targets rather than compiling each to zero tests, and the test recipes pass
  `--all-features`, so `just test` and the pre-commit hook run 101 tests and 5
  doc-tests where they previously ran 28. CI gained a test job of its own — the
  suite had only ever run as a side effect of `cargo llvm-cov` — a clippy pass
  over the featureless build, and `macos-latest` on every matrix. The publish
  workflow ran the same three featureless commands, so the release gate is now
  the full suite too.

### Fixed

- A walk with nothing to overlap no longer spawns a task per entry. On Unix
  with no `sort_by` key, no `filter_entry` predicate and `follow_links` off,
  resolving an entry does no I/O at all — `d_type` and `d_ino` give the file
  type and inode, and `is_hidden` is a name check — yet every entry was still
  dispatched through `JoinSet` at the default concurrency of 32. Resolution now
  runs sequentially unless the configuration actually performs per-entry I/O.
  On Windows it always does, since `is_hidden` needs `file_attributes`.

- docs.rs now documents the asynchronous half of the crate. `async` is off by
  default, and docs.rs built with default features, so the module and all four
  of its types were absent from
  [0.3.0's documentation](https://docs.rs/walkthrough/0.3.0/walkthrough/)
  entirely. `[package.metadata.docs.rs]` now asks for every feature and for
  `--cfg docsrs`, which is also what turns the "Available on crate feature
  `async` only" and "Available on Unix only" labels on. `just doc` renders the
  same thing locally.

- Documentation links no longer break in any feature configuration.
  `rustdoc::broken_intra_doc_links` is denied at the crate root, and the sync
  half's cross-references into the `async` module are written as a `cfg_attr`
  pair — linked with the feature on, the same sentence with the name as plain
  code without it — since the module is not a link target in a build that
  excludes it. CI documents both configurations; `just doc-check` runs the same
  pair locally.

- The `async` module now carries an overview — the builder's relationship to
  `WalkDir`, a runnable example, and the three ways the asynchronous walker
  differs — rather than a one-line summary, and the crate root documents the
  feature flag that gates it.

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
