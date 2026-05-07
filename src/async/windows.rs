use std::{
    fs as std_fs,
    marker::PhantomData,
    os::windows::{
        fs::{FileTypeExt, MetadataExt},
        io::AsRawHandle,
    },
    path::PathBuf,
};

use tokio::fs;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, GetFileInformationByHandle,
};

use crate::{Ancestor, DirEntry, Error, r#async::state::Async};

impl DirEntry<Async> {
    pub(super) async fn metadata_impl(&self) -> Result<std_fs::Metadata, Error> {
        if self.follow_link {
            fs::metadata(&self.path).await
        } else {
            Ok(self.metadata.clone())
        }
        .map_err(|err| Error::from_entry(self, err))
    }

    pub(super) async fn is_hidden_impl(&self) -> bool {
        if let Ok(metadata) = self.metadata_impl().await
            && (metadata.file_attributes() & 0x2) != 0
        {
            return true;
        }
        false
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
        let metadata = if file_type.is_dir() || file_type.is_symlink_dir() && follow_link {
            let resolved = fs::metadata(&path)
                .await
                .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
            file_type = resolved.file_type();
            resolved
        } else {
            raw
        };
        Ok(Self {
            path,
            file_type,
            follow_link,
            depth,
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
        let metadata = if file_type.is_dir() || file_type.is_symlink_dir() && follow_link {
            let metadata = fs::metadata(&path)
                .await
                .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
            file_type = metadata.file_type();
            metadata
        } else {
            entry
                .metadata()
                .await
                .map_err(|err| Error::new_io_error(path.clone(), depth, err))?
        };

        Ok(Self {
            path,
            file_type,
            follow_link,
            depth,
            metadata,
            state: PhantomData,
        })
    }

    pub(crate) async fn ancestor(&self) -> Option<Ancestor> {
        // FILE_FLAG_BACKUP_SEMANTICS is required to open a directory handle
        // (including when the path resolves to a directory via a symlink).
        // Without it, CreateFile returns ERROR_ACCESS_DENIED for directories.
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(self.path())
            .await
            .ok()?;
        let handle = file.as_raw_handle();

        unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            if GetFileInformationByHandle(handle, &mut info) != 0 {
                let index = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);
                return Some(Ancestor {
                    volume: info.dwVolumeSerialNumber,
                    index,
                });
            }
        }
        None
    }
}
