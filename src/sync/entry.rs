use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

/// Directory entry.
#[derive(Debug, Clone)]
pub struct DirEntry {
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
}

impl DirEntry {
    /// Returns the path of this entry.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Consumes the entry and returns its path.
    pub fn into_path(self) -> PathBuf {
        self.path
    }

    /// Returns the file type of this entry.
    pub fn file_type(&self) -> fs::FileType {
        self.file_type
    }

    /// Returns `true` if this entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }

    /// Returns the depth of this entry relative to the traversal root.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Returns the file name of this entry.
    pub fn file_name(&self) -> &OsStr {
        self.path
            .file_name()
            .unwrap_or_else(|| self.path.as_os_str())
    }
}
