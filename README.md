# walkthrough

[![Crates.io](https://img.shields.io/crates/v/walkthrough.svg)](https://crates.io/crates/walkthrough)
[![docs.rs](https://docs.rs/walkthrough/badge.svg)](https://docs.rs/walkthrough)
[![CI](https://github.com/edgar-linton/walkthrough/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/edgar-linton/walkthrough/actions/workflows/rust-ci.yml)

A recursive directory iterator for Rust with depth control, symlink loop detection, sorting, and hidden-file filtering.

## Usage

```rust
use walkthrough::WalkDir;

for entry in WalkDir::new("./my_project").min_depth(1).max_depth(5).skip_hidden(true) {
    match entry {
        Ok(e)  => println!("{}", e.path().display()),
        Err(e) => eprintln!("error: {e}"),
    }
}
```

## Configuration

| Method               | Default   | Description                                      |
| -------------------- | --------- | ------------------------------------------------ |
| `min_depth(n)`       | `0`       | Skip entries shallower than `n`                  |
| `max_depth(n)`       | unlimited | Skip entries deeper than `n`                     |
| `follow_links(bool)` | `false`   | Follow symbolic links                            |
| `skip_hidden(bool)`  | `false`   | Omit dot-files and dot-directories               |
| `group_dir(bool)`    | `false`   | Yield directories before files at each level     |
| `sort_by(fn)`        | none      | Custom comparator for entries within a directory |

### Sorting example

```rust
use walkthrough::WalkDir;

let walker = WalkDir::new(".")
    .group_dir(true)
    .sort_by(|a, b| a.file_name().cmp(b.file_name()));
```

## How it works

The walker maintains a stack of open directory iterators. Each directory is read, optionally sorted, and pushed onto the stack. On backtracking the stack is popped, keeping memory proportional to the depth of the tree rather than its total size.

Symlink loop detection compares device/inode identifiers (or equivalent) of every ancestor in the current path. A loop is reported as an error and traversal continues.

## Development

[`just`](https://github.com/casey/just) is used to run common tasks:

```
just fmt              # check formatting
just clippy           # run linter
just test             # run all tests
just ci               # full CI suite
```

## Roadmap

- [ ] Async support (`tokio` / `async-std`)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
