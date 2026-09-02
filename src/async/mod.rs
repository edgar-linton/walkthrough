//! Asynchronous traversal.
//!
//! [`AsyncWalkDir`] mirrors [`WalkDir`](crate::WalkDir), adding
//! [`concurrency`](AsyncWalkDir::concurrency), [`limit`](AsyncWalkDir::limit)
//! and [`offset`](AsyncWalkDir::offset). `metadata` and `is_hidden` on
//! [`AsyncDirEntry`] are `async fn`, and [`AsyncWalkDir::sort_by`] takes an
//! async key rather than a comparator.
//!
//! ```no_run
//! # async fn run() -> walkthrough::Result<()> {
//! use walkthrough::r#async::AsyncWalkDir;
//!
//! let mut walker = AsyncWalkDir::new("./my_project")
//!     .max_depth(5)
//!     .walker()
//!     .await;
//!
//! while let Some(entry) = walker.next().await {
//!     println!("{}", entry?.path().display());
//! }
//! # Ok(())
//! # }
//! ```
mod state;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::fs;

pub use state::{Async, AsyncWalkDir, AsyncWalker};

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
