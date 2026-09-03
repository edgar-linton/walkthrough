use std::{
    ffi::OsStr,
    fs,
    marker::PhantomData,
    path::{Path, PathBuf},
};

use crate::Blocking;

/// Directory entry.
#[derive(Debug)]
pub struct DirEntry<T = Blocking> {
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

// Not derived: a derived impl would demand `T: Clone`, which neither state
// marker satisfies.
impl<T> Clone for DirEntry<T> {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            file_type: self.file_type,
            follow_link: self.follow_link,
            depth: self.depth,
            #[cfg(unix)]
            ino: self.ino,
            metadata: self.metadata.clone(),
            state: PhantomData,
        }
    }
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

    /// Returns the depth relative to the traversal root.
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

/// Whether ancestor bookkeeping may be skipped when `follow_links` is off.
///
/// True on Unix: reaching an already-visited directory requires traversing a
/// symlink, so with links unfollowed no cycle is reachable and the `stat` per
/// directory that identity costs buys nothing.
///
/// False on Windows: a directory junction can be reported as a directory
/// rather than a link, so a cycle through one stays reachable. A missed loop
/// walks forever.
#[cfg(unix)]
pub(crate) const LOOPS_NEED_FOLLOW_LINKS: bool = true;
#[cfg(windows)]
pub(crate) const LOOPS_NEED_FOLLOW_LINKS: bool = false;

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
