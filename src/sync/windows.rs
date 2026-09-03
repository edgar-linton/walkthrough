use std::{
    fs,
    marker::PhantomData,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
};

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, GetFileInformationByHandle,
};

use super::state::Blocking;
use crate::{Ancestor, DirEntry, Error};

impl DirEntry<Blocking> {
    pub(super) fn metadata_impl(&self) -> Result<fs::Metadata, Error> {
        // The constructor already resolved a followed symlink, so the stored
        // metadata is the one to report in every case.
        Ok(self.metadata.clone())
    }

    pub(super) fn is_hidden_impl(&self) -> bool {
        if let Ok(metadata) = self.metadata_impl()
            && (metadata.file_attributes() & 0x2) != 0
        {
            return true;
        }
        false
    }

    pub(crate) fn from_path(path: PathBuf, depth: usize, follow_link: bool) -> Result<Self, Error> {
        let raw = fs::symlink_metadata(&path)
            .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
        let mut file_type = raw.file_type();
        // Only a followed symlink needs resolving: for anything else
        // `symlink_metadata` already describes the file itself.
        let metadata = if file_type.is_symlink() && follow_link {
            let resolved =
                fs::metadata(&path).map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
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

    pub(crate) fn from_std(
        entry: &fs::DirEntry,
        depth: usize,
        follow_link: bool,
    ) -> Result<Self, Error> {
        let path = entry.path();
        let mut file_type = entry
            .file_type()
            .map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
        // Only a followed symlink needs resolving: the scan that produced the
        // entry already carries every other one's file type and metadata.
        let metadata = if file_type.is_symlink() && follow_link {
            let resolved =
                fs::metadata(&path).map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
            file_type = resolved.file_type();
            resolved
        } else {
            entry
                .metadata()
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

    pub(crate) fn ancestor(&self) -> Option<Ancestor> {
        ancestor_of(self.path())
    }
}

/// Identity of the directory at `path`, as far as it can be obtained.
pub(super) fn ancestor_of(path: &Path) -> Option<Ancestor> {
    // FILE_FLAG_BACKUP_SEMANTICS is required for a directory handle;
    // without it CreateFile returns ERROR_ACCESS_DENIED.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
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
