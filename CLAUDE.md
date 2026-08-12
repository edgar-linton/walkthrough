# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

[`just`](https://github.com/casey/just) drives everything (`just --list` for the full set):

```
just check          # cargo check --all-targets --all-features
just clippy         # lint (alias: just lint)
just fmt            # format in place
just test           # unit + doc + integration
just pre-commit     # fmt + clippy + test — run before committing
just ci             # mirrors GitHub Actions: ci-fmt, ci-clippy, ci-doc, test
```

**The `just test` recipes do not enable features, so `tests/async.rs` is compiled
out of them** — it is gated on `#![cfg(feature = "async")]`. Any change touching
`src/async/` must be verified with:

```
cargo test --all-features            # all 3 suites, both walkers
cargo test --all-features --test async test_async_sort_by   # one file / name filter
cargo test --all-features --doc AsyncWalkDir::sort_by       # one doctest
```

`just clippy` and `just check` do use `--all-features`, so type errors in async
code surface there; only test *execution* is missing.

## Architecture

A recursive directory iterator (depth bounds, symlink loop detection, sorting,
hidden-file filtering) with two parallel implementations — synchronous
(`std::fs`) and asynchronous (`tokio::fs`, behind the `async` feature) — that
deliberately share types rather than code.

### The two axes of duplication

Everything in `src/` is organised along two axes; know which one you are on
before editing.

1. **Sync vs. async** — `src/sync/` and `src/async/` are near-mirrors:
   `state.rs` (walker + `DirStream` + builder), `unix.rs`, `windows.rs`. The async
   side is not a wrapper; it is a second full implementation whose `next()` is an
   inherent `async fn` rather than an `Iterator::next`. A behavioural fix in one
   almost always needs the same fix in the other, plus a mirrored test in both
   `tests/sync.rs` and `tests/async.rs`.
2. **Unix vs. Windows** — per-platform `DirEntry` construction (`from_path`,
   `from_std`), `metadata_impl`, `is_hidden_impl`, and `ancestor`. The public API
   never exposes `#[cfg]`: `src/*/mod.rs` documents each method once and delegates
   to a private `*_impl`. Keep it that way — `#![deny(missing_docs)]` plus
   platform-split docs is how the API drifts.

Windows entries are unlike Unix ones in a way that bites: `DirEntry.metadata` is
a plain `fs::Metadata` on Windows (populated eagerly at construction) but a
`OnceCell<fs::Metadata>` on Unix (lazily filled, cached). Hidden-ness follows:
`FILE_ATTRIBUTE_HIDDEN` on Windows, a leading `.` byte on Unix. Loop detection
compares `dev`+`ino` on Unix and volume serial + file index on Windows, both
normalised into `Ancestor` ([src/entry.rs](src/entry.rs)).

### Typestate

`DirEntry<T>` ([src/entry.rs](src/entry.rs)) carries a `PhantomData<T>` marker,
`Sync` or `Async`, so one struct serves both walkers while `metadata()` and
`is_hidden()` are sync on `DirEntry<Sync>` and `async` on `DirEntry<Async>`.
The `Clone` impl is hand-written on purpose: `#[derive(Clone)]` would require
`T: Clone` of the marker, which neither marker implements, so the derived impl
applied to no entry at all (fixed in Unreleased — see [CHANGELOG.md](CHANGELOG.md)).

### Traversal

Both walkers hold a `Vec<DirStream>` stack of open directories, so memory is
proportional to depth, not tree size. `DirStream` has two variants:

- `Live` — holds the raw `ReadDir` handle and pulls one entry at a time. The
  default, unsorted path.
- `Sorted` — used when `sort_by` **or** `group_dir` is set; the directory is read
  to completion into a `Vec` first (`collect_sorted`), then ordered.

`push_dir` truncates `self.ancestors` to the entry's depth before pushing — that
truncation *is* the backtracking mechanism, so there is no explicit pop of
ancestors. Errors are yielded as items and traversal continues; a symlink loop is
an `ErrorKind::LoopDetected`, not a panic or an early stop.

Ordering is applied in a fixed sequence in `collect_sorted`: failed entries sort
ahead of everything (they never reach the comparator/key), then `sort_by`, then
`group_dir` last — so `group_dir` always wins over the user's ordering. Preserve
that order when touching either walker.

### The sort_by asymmetry

This is the one place the two builders intentionally differ, and it is heavily
documented in-tree ([src/iter.rs](src/iter.rs), [src/async/state.rs](src/async/state.rs),
[CHANGELOG.md](CHANGELOG.md)). Do not "unify" it:

- `WalkDir::sort_by` takes a **comparator**, `FnMut(&DirEntry, &DirEntry) -> Ordering`.
  Metadata is cached on the entry, so reading it from a comparator costs one
  `stat` per entry, not per comparison.
- `AsyncWalkDir::sort_by` takes a **key**, `Fn(Arc<DirEntry<Async>>) -> impl Future<Output: Ord>`.
  A comparator cannot await `metadata()`, and blocking inside one panics with
  "Cannot start a runtime from within a runtime". The walker awaits the key once
  per entry and does a decorate-sort-undecorate (`sort_by_key` in
  [src/async/state.rs](src/async/state.rs)). The entry is passed as an `Arc` so the
  key may hold it across an `await`, and is recovered with `Arc::unwrap_or_clone`
  afterwards so metadata the key resolved stays cached on the yielded entry.

The async `Sorter` boxes the *whole* sort step, not the key function — that is
what keeps `AsyncWalker` non-generic over `K`. It is `Send + Sync` because the
walker borrows it across an `await` and walks must stay `tokio::spawn`-able;
`tests/async.rs` pins this with spawnable / `LocalSet` / both-runtime-flavour tests.

## Conventions

- Edition 2024, and the code uses let-chains (`if x && let Some(y) = ...`) freely.
- Lints in [src/lib.rs](src/lib.rs) are strict: `deny(missing_docs)`,
  `deny(missing_debug_implementations)`, `deny(warnings)` under `cfg(test)`. Every
  public item needs a doc comment, and hand-written `Debug` impls are expected for
  types holding boxed closures (both `Sorter`s, both `DirStream`s).
- `rustfmt.toml` sets `group_imports = "StdExternalCrate"` and
  `format_code_in_doc_comments` — run `just fmt` rather than hand-aligning.
- Comments explain *why* a shape was chosen, not what the line does. Match that
  register; the existing ones are the house style.
- Tests are integration-level in `tests/`, built on `tempfile` tree fixtures
  (`basic_tree`, `symlink_tree`, `loop_tree`, `dangling_symlink_tree`) that exist in
  both files. Symlink fixtures return `Option<TempDir>` and the test no-ops on
  `None`, because symlink creation needs privileges on Windows — CI runs the full
  matrix on `ubuntu-latest` and `windows-latest`.
- [CHANGELOG.md](CHANGELOG.md) follows Keep a Changelog and is expected to be
  updated under `## [Unreleased]` for any user-visible change, with a
  before/after snippet for breaking ones.
- Commit messages are prose: a short subject, then paragraphs explaining the
  problem, the chosen shape, and the alternatives rejected. See `a9c76bf` for
  the reference.
