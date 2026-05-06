use std::{
    fs,
    os::windows::{
        fs::{FileTypeExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::PathBuf,
};

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, GetFileInformationByHandle,
};

use super::entry::DirEntry;
use crate::Error;

impl DirEntry {
    /// Returns the metadata for this entry.
    pub fn metadata(&self) -> Result<fs::Metadata, Error> {
        if self.follow_link {
            fs::metadata(&self.path)
        } else {
            Ok(self.metadata.clone())
        }
        .map_err(|err| Error::from_entry(self, err))
    }

    /// Returns `true` if this entry has the Windows hidden file attribute set.
    pub fn is_hidden(&self) -> bool {
        use std::os::windows::fs::MetadataExt;

        if let Ok(metadata) = self.metadata()
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
        let metadata = if file_type.is_dir() || file_type.is_symlink_dir() && follow_link {
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
        let metadata = if file_type.is_dir() || file_type.is_symlink_dir() && follow_link {
            let metadata =
                fs::metadata(&path).map_err(|err| Error::new_io_error(path.clone(), depth, err))?;
            file_type = metadata.file_type();
            metadata
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
        })
    }

    pub(crate) fn ancestor(&self) -> Option<Ancestor> {
        #[cfg(windows)]
        {
            // FILE_FLAG_BACKUP_SEMANTICS is required to open a directory handle
            // (including when the path resolves to a directory via a symlink).
            // Without it, CreateFile returns ERROR_ACCESS_DENIED for directories.
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(self.path())
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
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    pub(super) volume: u32,
    pub(super) index: u64,
}
