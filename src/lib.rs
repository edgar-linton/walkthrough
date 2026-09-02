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
//! * Sorting entries within a directory — by a comparator over the entry when
//!   walking synchronously, or by an asynchronously computed key when walking
//!   asynchronously, where metadata is only reachable through an `await`.
//! * Detecting symbolic link loops to prevent infinite recursion.
//! * Resolving a directory's entries concurrently when walking asynchronously,
//!   so a directory's latency is the longest of its entries' rather than the
//!   sum — which is the difference between milliseconds and seconds on a
//!   network mount.
//! * Paging an asynchronous walk with `limit` and `offset`, for a listing
//!   endpoint that has to expose the same pair as the rest of its API.
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

// Unsupported targets produce a clear compile error rather than a cryptic
// "method not found" cascade from the missing platform-specific impl modules.
#[cfg(not(any(unix, windows)))]
compile_error!("walkthrough only supports Unix and Windows targets");

mod entry;
mod error;
mod iter;
mod sync;

#[cfg(feature = "async")]
pub mod r#async;
#[cfg(feature = "async")]
pub use r#async::{Async, AsyncDirEntry, AsyncWalkDir, AsyncWalker};
pub(crate) use entry::Ancestor;
pub use entry::DirEntry;
pub use error::{Error, ErrorKind, Result};
pub use iter::WalkDir;
pub use sync::{Sync, Walker};
