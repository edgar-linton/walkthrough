use std::{
    ffi::OsStr,
    fs,
    marker::PhantomData,
    path::{Path, PathBuf},
};

use crate::Sync;

/// Directory entry.
#[derive(Debug, Clone)]
pub struct DirEntry<T = Sync> {
    pub(super) path: PathBuf,
    pub(super) file_type: fs::FileType,
    pub(super) follow_link: bool,
    pub(super) depth: usize,
    #[cfg(unix)]
    pub(super) ino: u64,
    #[cfg(unix)]
    pub(super) metadata: once_cell::sync::OnceCell<fs::Metadata>,
    #[cfg(windows)]
    pub(super) metadata: fs::Metadata,
    pub(super) state: PhantomData<T>,
}

impl<T> DirEntry<T> {
    /// Returns the path of this entry.
    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Consumes the entry and returns its path.
    #[inline]
    pub fn into_path(self) -> PathBuf {
        self.path
    }

    /// Returns the file type of this entry.
    #[inline]
    pub fn file_type(&self) -> fs::FileType {
        self.file_type
    }

    /// Returns `true` if this entry is a directory.
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }

    /// Returns the depth of this entry relative to the traversal root.
    #[inline]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Returns the file name of this entry.
    #[inline]
    pub fn file_name(&self) -> &OsStr {
        self.path
            .file_name()
            .unwrap_or_else(|| self.path.as_os_str())
    }
}

#[cfg(unix)]
impl std::os::unix::fs::DirEntryExt for DirEntry {
    fn ino(&self) -> u64 {
        self.ino
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub(super) dev: u64,
    pub(super) ino: u64,
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub(super) volume: u32,
    pub(super) index: u64,
}
