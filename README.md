# Walkthrough

[![Crates.io](https://img.shields.io/crates/v/walkthrough.svg)](https://crates.io/crates/walkthrough)
[![Documentation](https://docs.rs/walkthrough/badge.svg)](https://docs.rs/walkthrough)

A robust, performant, and highly configurable recursive directory iterator for Rust. This crate allows you to traverse file systems efficiently with fine-grained control over depth, sorting, and metadata.

## ✨ Features

- 🚀 **Lazy Iteration** – Results are computed on the fly, keeping memory usage minimal even for massive trees.
- 🛡️ **Cycle Protection** – Built-in symbolic link loop detection to prevent infinite recursion.
- 🔧 **Highly Configurable** – Precise control over `min_depth`, `max_depth`, and hidden file visibility.
- 📂 **Smart Ordering** – Support for custom sorting and grouping directories before files.

## Examples

### Basic

```rust
use walkthrough::WalkDir;

fn main() {
    let walker = WalkDir::new("./my_project")
        .min_depth(1)
        .max_depth(5)
        .skip_hidden(true);

    for entry in walker {
        match entry {
            Ok(entry) => println!("Path: {}", entry.path().display()),
            Err(err) => eprintln!("Error walking directory: {}", err),
        }
    }
}
```

### Sorting & Grouping

You can group directories first or provide a custom sorting function:

```rust
let walker = WalkDir::new(".")
    .group_dir(true) // Directories appear before files
    .sort_by(|a, b| a.file_name().cmp(b.file_name())); // Alphabetic sort
```

## 🧠 How it Works

The walker uses a stack-based approach to navigate the file system tree. It yields the root entry first and then descends into subdirectories based on your configuration.

- Loop Detection: When follow_links is enabled, the walker tracks directory ancestors using unique identifiers to prevent getting stuck in symlink cycles.
- Depth Control: The max_depth and min_depth filters ensure you only see the parts of the tree you are interested in.

## ⚖️ License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
