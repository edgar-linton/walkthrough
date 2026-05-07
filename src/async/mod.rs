//! Asynchronous implementation.
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
    /// When `follow_links` is enabled the metadata reflects the symlink target;
    /// otherwise it reflects the symlink itself.
    pub async fn metadata(&self) -> Result<fs::Metadata, Error> {
        self.metadata_impl().await
    }

    /// Returns `true` if this entry is considered hidden by the operating system.
    pub async fn is_hidden(&self) -> bool {
        self.is_hidden_impl().await
    }
}
