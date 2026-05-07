use std::{
    fs,
    marker::PhantomData,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirEntryExt, MetadataExt},
    },
    path::PathBuf,
};

use once_cell::sync::OnceCell;

use crate::{Ancestor, DirEntry, Error};

impl DirEntry {
    pub(super) fn metadata_impl(&self) -> Result<fs::Metadata, Error> {
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

    pub(super) fn is_hidden_impl(&self) -> bool {
        self.file_name().as_bytes().starts_with(b".")
    }

    pub(crate) fn from_path(path: PathBuf, depth: usize, follow_link: bool) -> Result<Self, Error> {
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
            state: PhantomData,
        })
    }

    pub(crate) fn from_std(
        entry: &fs::DirEntry,
        depth: usize,
        follow_link: bool,
    ) -> Result<Self, Error> {
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
            state: PhantomData,
        })
    }

    pub(crate) fn ancestor(&self) -> Option<Ancestor> {
        let metadata = self.metadata_impl().ok()?;
        Some(Ancestor {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
}
