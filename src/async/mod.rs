//! Asynchronous traversal.
//!
//! ```no_run
//! # async fn run() -> walkthrough::Result<()> {
//! use walkthrough::r#async::AsyncWalkDir;
//!
//! let mut walker = AsyncWalkDir::new("./my_project").max_depth(5).walker();
//!
//! while let Some(entry) = walker.next_entry().await {
//!     println!("{}", entry?.path().display());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Runtime
//!
//! This module is tokio-specific: it reads through `tokio::fs` and spawns onto
//! the current tokio runtime, which has to be running. Implementing
//! `futures_core::Stream` does not make it portable to another executor.
//!
//! # Choosing between the walkers
//!
//! Neither walker reaches the filesystem faster than the other. `tokio::fs`
//! hands every operation to a blocking thread pool, and one traversal is
//! thousands of those handovers, so on local storage
//! [`WalkDir`](crate::WalkDir) finishes a walk in a fraction of the time.
//!
//! Keeping a request handler off its worker thread is therefore not on its own
//! a reason to walk asynchronously — [`WalkDir`](crate::WalkDir) inside
//! `tokio::task::spawn_blocking` does that with a single handover for the
//! whole walk:
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use std::path::PathBuf;
//!
//! use walkthrough::WalkDir;
//!
//! let root = PathBuf::from("./my_project");
//! let entries =
//!     tokio::task::spawn_blocking(move || WalkDir::new(root).into_iter().collect::<Vec<_>>())
//!         .await?;
//! # Ok(())
//! # }
//! ```
//!
//! Reach for this module when something other than the walk itself asks for
//! it:
//!
//! - entries are consumed as they arrive, so a response can start before the
//!   traversal ends;
//! - the walk has to end when its caller does — dropping an [`AsyncWalker`]
//!   stops it, where a blocking task runs to completion;
//! - [`filter_entry`](AsyncWalkDir::filter_entry) or
//!   [`sort_by`](AsyncWalkDir::sort_by) awaits something slow, which is also
//!   where [`concurrency`](AsyncWalkDir::concurrency) earns its keep;
//! - the tree is on a network filesystem, where per-entry latency dominates.
mod concurrent;
mod iter;
mod state;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::fs;

pub use iter::{AsyncWalkDir, Filtering};
pub use state::{Async, AsyncWalker};

/// Type alias for [`DirEntry<Async>`](crate::DirEntry).
pub type AsyncDirEntry = crate::DirEntry<Async>;

use crate::{DirEntry, Error, Result};

impl DirEntry<Async> {
    /// Returns the metadata for this entry.
    ///
    /// Reflects the symlink target when `follow_links` is enabled, the symlink
    /// itself otherwise.
    pub async fn metadata(&self) -> Result<fs::Metadata, Error> {
        self.metadata_impl().await
    }

    /// Returns `true` if the operating system considers this entry hidden.
    pub async fn is_hidden(&self) -> bool {
        self.is_hidden_impl().await
    }
}
