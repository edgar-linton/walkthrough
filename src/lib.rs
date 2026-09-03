#![deny(missing_docs)]
#![deny(missing_debug_implementations)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(test, deny(warnings))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(rustdoc::broken_intra_doc_links)]

//! Recursive directory traversal.
//!
//! [`WalkDir`] walks synchronously. With the `async` feature,
//! `walkthrough::r#async` walks asynchronously, resolving each directory's
//! entries concurrently.
//!
//! Both offer min/max depth, symlink following, symlink-loop detection,
//! hidden-file filtering, sorting, and directory grouping.
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

#[cfg(not(any(unix, windows)))]
compile_error!("walkthrough only supports Unix and Windows targets");

mod entry;
mod error;
mod iter;
mod sync;

#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod r#async;
pub use entry::DirEntry;
pub(crate) use entry::{Ancestor, LOOPS_NEED_FOLLOW_LINKS};
pub use error::{Error, ErrorKind, Result};
pub use iter::WalkDir;
pub use sync::{Blocking, Walker};
