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

Then drive the walker manually with `.next_entry().await`:

```rust
use walkthrough::r#async::AsyncWalkDir;

let mut walker = AsyncWalkDir::new("./my_project")
    .min_depth(1)
    .max_depth(5)
    .skip_hidden(true)
    .walker();

while let Some(entry) = walker.next_entry().await {
    match entry {
        Ok(e)  => println!("{}", e.path().display()),
        Err(e) => eprintln!("error: {e}"),
    }
}
```

`AsyncWalker` also implements `futures_core::Stream`, so the combinators of
`futures::StreamExt` or `tokio_stream::StreamExt` apply to it directly:

```rust
use futures::StreamExt;
use walkthrough::r#async::AsyncWalkDir;

let paths: Vec<_> = AsyncWalkDir::new("./my_project")
    .walker()
    .filter_map(|entry| async move { entry.ok() })
    .map(|entry| entry.into_path())
    .collect()
    .await;
```

`walker()` does no I/O, so an unreadable root arrives as the first `Err` rather than
failing at construction.

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

`sort_by`'s argument differs between the walkers: a comparator in one, an async key in
the other.

`AsyncWalkDir` has three more:

| Method             | Default   | Description                                            |
| ------------------ | --------- | ------------------------------------------------------ |
| `concurrency(n)`   | `32`      | Entries of one directory resolved at once              |
| `filter_entry(fn)` | none      | Decide each entry's fate before descending into it     |
| `limit(n)`         | unlimited | Yield at most `n` entries, then end the walk           |
| `offset(n)`        | `0`       | Skip the first `n` entries the walk would yield        |

### Sorting

`WalkDir::sort_by` takes a comparator; `metadata` is cached, so reading it there costs
one `stat` per entry, not one per comparison:

```rust
use walkthrough::WalkDir;

let walker = WalkDir::new(".")
    .group_dir(true)
    .sort_by(|a, b| a.file_name().cmp(b.file_name()));
```

`AsyncWalkDir::sort_by` takes a key, awaited once per entry before ordering, since a
comparator cannot await `metadata`. Tuple keys give compound orderings,
`std::cmp::Reverse` descending order:

```rust
use walkthrough::r#async::AsyncWalkDir;

// Directories first, then smallest file first, ties broken by name.
let walker = AsyncWalkDir::new(".")
    .sort_by(|entry| async move {
        let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
        (!entry.is_dir(), size, entry.file_name().to_owned())
    })
    .walker();
```

The entry arrives as an `Arc`, so the key may hold it across an `await`, and metadata
it resolves stays cached on the yielded entry.

The order is total, in precedence order: `group_dir`, then entries that failed
outright, then the key, then the file name.

### Concurrency

`concurrency` bounds every per-entry resolve: the `sort_by` key, the file type on a
filesystem without `d_type`, and the hidden flag on Windows. It never affects order.

The win scales with per-entry latency. Locally, 2200 entries ordered by size take
38.7 ms at `concurrency(1)` and 15.2 ms at the default 32. On a network mount 128–512
is reasonable: 2000 entries at 5 ms each take 10.2 s sequentially, 175 ms at 64.

```rust
use walkthrough::r#async::AsyncWalkDir;

let walker = AsyncWalkDir::new("/mnt/share")
    .max_depth(1)
    .concurrency(256)
    .sort_by(|entry| async move { entry.metadata().await.map(|m| m.len()).unwrap_or(0) })
    .walker();
```

### Filtering

`filter_entry` runs before the walker descends, so `Filtering::IgnoreDir` prunes a
subtree without reading it. The predicate is async, so it may await `metadata`:

```rust
use std::sync::Arc;

use walkthrough::r#async::{AsyncDirEntry, AsyncWalkDir, Filtering};

let mut walker = AsyncWalkDir::new("./my_project")
    .filter_entry(|entry: Arc<AsyncDirEntry>| async move {
        if entry.file_name() == "target" {
            Filtering::IgnoreDir
        } else {
            Filtering::Continue
        }
    })
    .walker();
```

`Filtering::IgnoreEntry` drops the entry but still descends into it. To drop entries
from the output without affecting descent at all, use `StreamExt::filter` instead —
pruning the walk is the part a downstream combinator cannot express.

### Paging

`limit` and `offset` are `skip(offset).take(limit)` over the walk the same builder
would otherwise produce:

```rust
use walkthrough::r#async::AsyncWalkDir;

let walker = AsyncWalkDir::new(root)
    .max_depth(1)
    .offset(200)
    .limit(100)
    .sort_by(|entry| async move { entry.metadata().await.map(|m| m.len()).unwrap_or(0) })
    .walker();
```

They count yielded entries across the whole traversal, not per directory. A skipped
directory is still descended into, and an offset past the end yields nothing.

Set an ordering when paginating: filesystem order is not reproducible across two
`read_dir` calls, so an offset over an unordered walk is not a stable position.

## How it works

The walker maintains a stack of open directory handles. In the unsorted case the handle is read in batches of `concurrency` entries, which are resolved together and drained one at a time (`DirStream::Live`); when sorting is configured all entries are collected, resolved `concurrency` at a time, and ordered first (`DirStream::Sorted`). On backtracking the stack is popped, keeping memory proportional to tree depth rather than total size.

Symlink loop detection compares device/inode identifiers (or equivalent) of every ancestor in the current path. A loop is reported as an error and traversal continues.

## Development

The toolchain is pinned nightly in `rust-toolchain.toml`, required by the options in
`rustfmt.toml`; `rustup` fetches it automatically.

Local tasks run through [`just`](https://github.com/casey/just); CI runs its own
commands directly (see `.github/workflows/`):

```
just fmt              # format code in place
just clippy           # run linter
just test             # run all tests
just doc              # docs as docs.rs renders them
just pre-commit       # format, lint, and test — also a git pre-commit hook
```

The hook is installed by `cargo-husky` on the first `cargo test` after cloning.

To cut a release: bump `Cargo.toml`, add a dated entry to `CHANGELOG.md`, commit, then
`just tag`.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
