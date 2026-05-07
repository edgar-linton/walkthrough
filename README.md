# walkthrough

[![Crates.io](https://img.shields.io/crates/v/walkthrough.svg)](https://crates.io/crates/walkthrough)
[![docs.rs](https://docs.rs/walkthrough/badge.svg)](https://docs.rs/walkthrough)
[![CI](https://github.com/edgar-linton/walkthrough/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/edgar-linton/walkthrough/actions/workflows/rust-ci.yml)

A recursive directory iterator for Rust with depth control, symlink loop detection, sorting, and hidden-file filtering. Supports both synchronous and asynchronous traversal.

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
walkthrough = { version = "0.2", features = ["async"] }
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
| `sort_by(fn)`        | none      | Custom comparator for entries within each directory |

### Sorting example

```rust
use walkthrough::WalkDir;

let walker = WalkDir::new(".")
    .group_dir(true)
    .sort_by(|a, b| a.file_name().cmp(b.file_name()));
```

## How it works

The walker maintains a stack of open directory handles. In the unsorted case the handle is read one entry at a time (`DirStream::Live`); when sorting is configured all entries are collected first (`DirStream::Sorted`). On backtracking the stack is popped, keeping memory proportional to tree depth rather than total size.

Symlink loop detection compares device/inode identifiers (or equivalent) of every ancestor in the current path. A loop is reported as an error and traversal continues.

## Development

[`just`](https://github.com/casey/just) is used to run common tasks:

```
just fmt              # check formatting
just clippy           # run linter
just test             # run all tests
just ci               # full CI suite
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
