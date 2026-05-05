use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use crate::Error;

/// Directory entry.
#[derive(Debug, Clone)]
pub struct DirEntry {
    path: PathBuf,
    file_type: fs::FileType,
    follow_link: bool,
    depth: usize,
    #[cfg(unix)]
    ino: u64,
    #[cfg(not(windows))]
    metadata: std::cell::OnceCell<fs::Metadata>,
    #[cfg(windows)]
    metadata: fs::Metadata,
}

impl DirEntry {
    /// Returns a reference to the underlying path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Consumes the [`DirEntry`] and returns the owned path.
    pub fn into_path(self) -> PathBuf {
        self.path
    }

    /// File type.
    pub fn file_type(&self) -> fs::FileType {
        self.file_type
    }

    /// Is directory.
    pub fn is_dir(&self) -> bool {
        self.file_type.is_dir()
    }

    /// Depth.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// File name
    pub fn file_name(&self) -> &OsStr {
        self.path
            .file_name()
            .unwrap_or_else(|| self.path.as_os_str())
    }

    /// Returns a reference of the underlying metadata.
    #[cfg(windows)]
    pub fn metadata(&self) -> Result<fs::Metadata, Error> {
        if self.follow_link {
            fs::metadata(&self.path)
        } else {
            Ok(self.metadata.clone())
        }
        .map_err(|err| Error::from_entry(self, err))
    }

    /// Returns a reference of the underlying metadata.
    #[cfg(unix)]
    pub fn metadata(&self) -> Result<fs::Metadata, Error> {
        self.metadata
            .try_get_init(|| {
                if self.follow_link {
                    fs::metadata(self.path())
                } else {
                    fs::symlink_metadata(self.path())
                }
                .map_err(|err| Error::from_entry(self, err))
            })
            .to_owned()
    }

    /// File is hidden.
    #[cfg(windows)]
    pub fn is_hidden(&self) -> bool {
        use std::os::windows::fs::MetadataExt;

        if let Some(name) = self.file_name().to_str()
            && name.starts_with('.')
        {
            return true;
        }

        if let Ok(metadata) = self.metadata()
            && (metadata.file_attributes() & 0x2) != 0
        {
            return true;
        }
        false
    }

    /// File is hidden.
    #[cfg(unix)]
    pub fn is_hidden(&self) -> bool {
        use std::os::unix::ffi::OsStrExt;

        self.file_name().as_bytes().starts_with(b'.')
    }

    #[cfg(windows)]
    pub(crate) fn from_std(
        entry: &fs::DirEntry,
        depth: usize,
        follow_link: bool,
    ) -> Result<Self, Error> {
        use std::os::windows::fs::FileTypeExt;

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

    #[cfg(unix)]
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
        let mut metadata = OnceCell::new();
        if file_type.is_dir() || entry.is_symlink() && follow_link {
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

    #[cfg(windows)]
    pub(crate) fn ancestor(&self) -> Option<Ancestor> {
        use std::os::windows::fs::MetadataExt;

        let metadata = &self.metadata;
        match (metadata.volume_serial_number(), metadata.file_index()) {
            (Some(volume), Some(index)) => Some(Ancestor { volume, index }),
            _ => None,
        }
    }

    #[cfg(unix)]
    pub(crate) fn ancestor(&self) -> Option<Ancestor> {
        use std::os::unix::fs::MetadataExt;

        self.metadata.get().map(|m| Ancestor::new(m.dev(), m.ino()))
    }
}

#[cfg(unix)]
impl std::os::unix::fs::DireEntryExt for DirEntry {
    fn ino(&self) -> u64 {
        self.ino
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    volume: u32,
    index: u64,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ancestor {
    dev: u64,
    ino: u64,
}
