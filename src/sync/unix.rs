use std::{fs, path::PathBuf};

use crate::Error;

use super::entry::DirEntry;

impl DirEntry {
    /// Returns the metadata for this entry.
    pub fn metadata(&self) -> Result<fs::Metadata, Error> {
        self.metadata
            .get_or_try_init(|| {
                if self.follow_link {
                    fs::metadata(self.path())
                } else {
                    fs::symlink_metadata(self.path())
                }
                .map_err(|err| Error::from_entry(self, err))
            })
            .map(|m| m.to_owned())
    }

    /// Returns `true` if this entry is hidden (name starts with `.`).
    pub fn is_hidden(&self) -> bool {
        use std::os::unix::ffi::OsStrExt;

        self.file_name().as_bytes().starts_with(b".")
    }

    pub(crate) fn from_path(path: PathBuf, depth: usize, follow_link: bool) -> Result<Self, Error> {
        use once_cell::sync::OnceCell;
        use std::os::unix::fs::MetadataExt;

        let raw = fs::symlink_metadata(&path)
            .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
        let mut file_type = raw.file_type();
        let mut ino = raw.ino();
        let metadata = OnceCell::new();
        if file_type.is_dir() || file_type.is_symlink() && follow_link {
            let resolved =
                fs::metadata(&path).map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
            file_type = resolved.file_type();
            ino = resolved.ino();
            metadata.set(resolved).unwrap();
        } else {
            metadata.set(raw).unwrap();
        }
        Ok(Self {
            path,
            file_type,
            follow_link,
            depth,
            ino,
            metadata,
        })
    }

    pub(crate) fn from_std(
        entry: &fs::DirEntry,
        depth: usize,
        follow_link: bool,
    ) -> Result<Self, Error> {
        use once_cell::sync::OnceCell;
        use std::os::unix::fs::{DirEntryExt, MetadataExt};

        let path = entry.path();
        let mut file_type = entry
            .file_type()
            .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
        let mut ino = entry.ino();
        let metadata = OnceCell::new();
        if file_type.is_dir() || file_type.is_symlink() && follow_link {
            let resolved_metadata =
                fs::metadata(&path).map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
            file_type = resolved_metadata.file_type();
            ino = resolved_metadata.ino();
            metadata.set(resolved_metadata).unwrap();
        }

        Ok(Self {
            path,
            file_type,
            follow_link,
            depth,
            ino,
            metadata,
        })
    }

    pub(crate) fn ancestor(&self) -> Option<Ancestor> {
        use std::os::unix::fs::MetadataExt;

        let metadata = self.metadata().ok()?;
        Some(Ancestor {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
}

impl std::os::unix::fs::DirEntryExt for DirEntry {
    fn ino(&self) -> u64 {
        self.ino
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub(super) dev: u64,
    pub(super) ino: u64,
}
