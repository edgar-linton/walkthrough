use std::{
    fs as std_fs,
    marker::PhantomData,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirEntryExt, MetadataExt},
    },
    path::PathBuf,
};

use once_cell::sync::OnceCell;
use tokio::fs;

use super::state::Async;
use crate::{Ancestor, DirEntry, Error};

impl DirEntry<Async> {
    pub(super) async fn metadata_impl(&self) -> Result<std_fs::Metadata, Error> {
        // Return the cached value if a previous call already resolved it.
        if let Some(m) = self.metadata.get() {
            return Ok(m.clone());
        }
        let m = if self.follow_link {
            fs::metadata(self.path()).await
        } else {
            fs::symlink_metadata(self.path()).await
        }
        .map_err(|err| Error::from_entry(self, err))?;
        // Best-effort cache; ignore the race if another task set it first.
        let _ = self.metadata.set(m.clone());
        Ok(m)
    }

    pub(super) async fn is_hidden_impl(&self) -> bool {
        self.file_name().as_bytes().starts_with(b".")
    }

    pub(crate) async fn from_path(
        path: PathBuf,
        depth: usize,
        follow_link: bool,
    ) -> Result<Self, Error> {
        let raw = fs::symlink_metadata(&path)
            .await
            .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
        let mut file_type = raw.file_type();
        let mut ino = raw.ino();
        let metadata = OnceCell::new();
        if file_type.is_dir() || file_type.is_symlink() && follow_link {
            let resolved = fs::metadata(&path)
                .await
                .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
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

    pub(crate) async fn from_std(
        entry: &fs::DirEntry,
        depth: usize,
        follow_link: bool,
    ) -> Result<Self, Error> {
        let path = entry.path();
        let mut file_type = entry
            .file_type()
            .await
            .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
        let mut ino = entry.ino();
        let metadata = OnceCell::new();
        if file_type.is_dir() || file_type.is_symlink() && follow_link {
            let resolved = fs::metadata(&path)
                .await
                .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
            file_type = resolved.file_type();
            ino = resolved.ino();
            metadata.set(resolved).unwrap();
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

    pub(crate) async fn ancestor(&self) -> Option<Ancestor> {
        let metadata = self.metadata_impl().await.ok()?;
        Some(Ancestor {
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }
}

impl DirEntryExt for DirEntry<Async> {
    fn ino(&self) -> u64 {
        self.ino
    }
}
