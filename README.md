# walkthrough

[![Crates.io](https://img.shields.io/crates/v/walkthrough.svg)](https://crates.io/crates/walkthrough)
[![docs.rs](https://docs.rs/walkthrough/badge.svg)](https://docs.rs/walkthrough)
[![CI](https://github.com/edgar-linton/walkthrough/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/edgar-linton/walkthrough/actions/workflows/rust-ci.yml)

A recursive directory iterator for Rust with depth control, symlink loop detection, sorting, and hidden-file filtering. Supports both synchronous and asynchronous traversal, with concurrent metadata resolution and `limit`/`offset` paging on the async side.

## Usage

### Synchronous

```rust
use walkthrough::WalkDir;

for entry in WalkDir::new("./my_project").min_depth(1).max_depth(5).skip_hidden(true) {
    match entry {
        Ok(e)  => println!("{}", e.path().display()),
        Err(e) => eprintln!("error: {e}"),
    }
}
```

### Asynchronous

Enable the `async` feature in `Cargo.toml`:

```toml
[dependencies]
walkthrough = { version = "0.4", features = ["async"] }
```

Then drive the walker manually with `.next().await`:

```rust
use walkthrough::AsyncWalkDir;

let mut walker = AsyncWalkDir::new("./my_project")
    .min_depth(1)
    .max_depth(5)
    .skip_hidden(true)
    .walker()
    .await;

while let Some(entry) = walker.next().await {
    match entry {
        Ok(e)  => println!("{}", e.path().display()),
        Err(e) => eprintln!("error: {e}"),
    }
}
```

## Configuration

Both `WalkDir` and `AsyncWalkDir` expose the same builder methods:

| Method               | Default   | Description                                      |
| -------------------- | --------- | ------------------------------------------------ |
| `min_depth(n)`       | `0`       | Skip entries shallower than `n`                  |
| `max_depth(n)`       | unlimited | Skip entries deeper than `n`                     |
| `follow_links(bool)` | `false`   | Follow symbolic links                            |
| `skip_hidden(bool)`  | `false`   | Omit dot-files and dot-directories               |
| `group_dir(bool)`    | `false`   | Yield directories before files at each level     |
| `sort_by(fn)`        | none      | Order the entries within each directory           |

`sort_by` is the one method whose argument differs between the two walkers, because
metadata is reachable synchronously in one and only through an `await` in the other.

`AsyncWalkDir` has three more, which the synchronous walker has no use for — it has
no I/O to overlap, and its iterator composes with `Iterator::skip` and `take`:

| Method             | Default   | Description                                            |
| ------------------ | --------- | ------------------------------------------------------ |
| `concurrency(n)`   | `32`      | Entries of one directory resolved at once              |
| `limit(n)`         | unlimited | Yield at most `n` entries, then end the walk           |
| `offset(n)`        | `0`       | Skip the first `n` entries the walk would yield        |

### Sorting

`WalkDir::sort_by` takes a comparator. `metadata` is cached on the entry, so reading
it from there costs one `stat` per entry rather than one per comparison:

```rust
use walkthrough::WalkDir;

let walker = WalkDir::new(".")
    .group_dir(true)
    .sort_by(|a, b| a.file_name().cmp(b.file_name()));
```

`AsyncWalkDir::sort_by` takes a key instead, computed asynchronously and awaited once
per entry before ordering. A comparator would be the wrong shape there: it cannot
await `metadata`, and blocking on it from inside one panics with "Cannot start a
runtime from within a runtime" — a `500` if the walk happens in a request handler.
Compound orderings are tuple keys, and descending order is `std::cmp::Reverse`:

```rust
use walkthrough::AsyncWalkDir;

// Directories first, then smallest file first, ties broken by name.
let walker = AsyncWalkDir::new(".")
    .sort_by(|entry| async move {
        let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
        (!entry.is_dir(), size, entry.file_name().to_owned())
    })
    .walker()
    .await;
```

The entry arrives as an `Arc`, so the key may hold it across an `await` and metadata
it resolves stays cached on the entry the walk yields.

The resulting order is total, and is, in precedence order: `group_dir` if set, then
entries that failed outright — for which no key is computed — then the key, then the
file name. The name tie-break is what makes it reproducible between two walks of the
same directory, which is what `offset` needs.

### Concurrency

Every per-entry resolve is bounded by `concurrency`: the `sort_by` key, the file type
on a filesystem that does not populate `d_type`, and the hidden flag on Windows.
Without it those are awaited one entry at a time, so a directory's latency is the sum
of its entries' rather than the longest of them.

The size of the win is the per-entry latency. On a local filesystem, 2200 entries
ordered by size take 38.7 ms at `concurrency(1)` and 15.2 ms at the default 32, bound
from there by Tokio's blocking-pool dispatch rather than the syscall. A network mount
or FUSE layer where one `stat` costs milliseconds is bound by round trips instead, and
128–512 is reasonable there — 2000 entries at 5 ms each take 10.2 s sequentially and
175 ms at `concurrency(64)`:

```rust
use walkthrough::AsyncWalkDir;

let walker = AsyncWalkDir::new("/mnt/share")
    .max_depth(1)
    .concurrency(256)
    .sort_by(|entry| async move { entry.metadata().await.map(|m| m.len()).unwrap_or(0) })
    .walker()
    .await;
```

Concurrency never affects the order entries are yielded in — results are placed by
directory position, not by completion.

`examples/latency_probe.rs` measures all of this against a real directory, which is
worth doing before tuning: if `names+stat` comes back in milliseconds, the latency is
not in the filesystem and `concurrency` will not move it.

### Paging

`limit` and `offset` are `skip(offset).take(limit)` over the walk the same builder
would otherwise produce, so a listing endpoint can expose the same pair as the rest of
an SQL-backed API:

```rust
use walkthrough::AsyncWalkDir;

let walker = AsyncWalkDir::new(root)
    .max_depth(1)
    .offset(200)
    .limit(100)
    .sort_by(|entry| async move { entry.metadata().await.map(|m| m.len()).unwrap_or(0) })
    .walker()
    .await;
```

They count yielded entries across the whole traversal rather than per directory, so
they compose with `min_depth` and `skip_hidden` the way `LIMIT` composes with a `WHERE`
clause. A skipped directory is still descended into, and an offset past the end yields
nothing rather than an error.

Set an ordering when paginating: filesystem order is not reproducible across two
`read_dir` calls of the same directory, so an offset over an unordered walk is not a
stable position. And note that `limit` bounds what a caller receives, not what the
filesystem is asked — ordering by a key still requires the key of every candidate, so
`concurrency` is what makes a page fast.

## How it works

The walker maintains a stack of open directory handles. In the unsorted case the handle is read in batches of `concurrency` entries, which are resolved together and drained one at a time (`DirStream::Live`); when sorting is configured all entries are collected, resolved `concurrency` at a time, and ordered first (`DirStream::Sorted`). On backtracking the stack is popped, keeping memory proportional to tree depth rather than total size.

Symlink loop detection compares device/inode identifiers (or equivalent) of every ancestor in the current path. A loop is reported as an error and traversal continues.

## Development

The project pins a nightly Rust toolchain via `rust-toolchain.toml` (needed for
`rustfmt`'s doc-comment, string, and import-grouping options in `rustfmt.toml`);
`rustup` fetches it automatically on first use.

[`just`](https://github.com/casey/just) is used to run common local dev tasks;
CI runs its own checks directly rather than through `just` (see
`.github/workflows/`):

```
just fmt              # format code in place
just clippy           # run linter
just test             # run all tests
just pre-commit       # format, lint, and test — run before committing
```

`just pre-commit` also runs automatically as a git pre-commit hook, installed by
`cargo-husky` the first time you run `cargo test` after cloning — no separate setup
step required.

To cut a release: bump the version in `Cargo.toml`, add a dated entry to
`CHANGELOG.md`, commit, then `just tag` to tag and push — the publish workflow
verifies the tag, re-runs CI, and publishes to crates.io.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
