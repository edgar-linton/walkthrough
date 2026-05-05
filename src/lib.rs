#![deny(missing_docs)]
#![deny(missing_debug_implementations)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(test, deny(warnings))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # Recursive Directory Traversal
//!
//! This crate provides a flexible and efficient way to recursively walk through
//! directories. It handles common tasks like:
//!
//! * Filtering hidden files.
//! * Setting minimum and maximum recursion depth.
//! * Following or ignoring symbolic links.
//! * Sorting entries within a directory.
//! * Detecting symbolic link loops to prevent infinite recursion.
//!
//! ## Example
//!
//! ```no_run
//! use walkthrough::WalkDir;
//!
//! for entry in WalkDir::new("src").max_depth(2) {
//!     match entry {
//!         Ok(e) => println!("Found: {:?}", e.path()),
//!         Err(err) => eprintln!("Error: {}", err),
//!     }
//! }
//! ```
mod error;
mod sync;

pub use error::{Error, ErrorKind, Result};
pub use sync::{DirEntry, WalkDir, Walker};
