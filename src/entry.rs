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

#[cfg(windows)]
impl<T> DirEntry<T> {
    /// Whether this entry is a reparse point.
    ///
    /// A cycle on Windows needs one somewhere along the path: NTFS allows hard
    /// links to files only, so a tree of plain directories cannot close a
    /// loop. The attribute comes with the directory scan, so the answer costs
    /// nothing — but it describes the entry itself, and resolving a followed
    /// symlink replaces the stored metadata with the target's, which is a
    /// plain directory. Only meaningful for an unfollowed entry.
    pub(crate) fn is_reparse_point(&self) -> bool {
        use std::os::windows::fs::MetadataExt;

        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        self.metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
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
