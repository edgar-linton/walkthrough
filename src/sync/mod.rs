mod state;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::fs;

pub use state::{Sync, Walker};

use crate::{DirEntry, Error, Result};

impl DirEntry<Sync> {
    /// Returns the metadata for this entry.
    ///
    /// Reflects the symlink target when `follow_links` is enabled, the symlink
    /// itself otherwise.
    pub fn metadata(&self) -> Result<fs::Metadata, Error> {
        self.metadata_impl()
    }

    /// Returns `true` if the operating system considers this entry hidden.
    pub fn is_hidden(&self) -> bool {
        self.is_hidden_impl()
    }
}
