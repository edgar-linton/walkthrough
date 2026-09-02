use std::{
    io,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::DirEntry;

/// Distinguishes the cause of a traversal [`struct@Error`].
#[derive(Debug, Error)]
pub enum ErrorKind {
    /// Reading a directory entry or its metadata failed.
    #[error("{0}")]
    Io(#[source] io::Error),

    /// Following a symlink would revisit an ancestor directory.
    #[error("symlink loop detected")]
    LoopDetected,
}

/// Error produced during directory traversal, carrying the
/// [`path`](Self::path), [`depth`](Self::depth) and [`kind`](Self::kind).
#[derive(Debug, Error)]
#[error("Error on {path} with depth {depth} and kind: {kind}")]
pub struct Error {
    path: PathBuf,
    depth: usize,
    kind: ErrorKind,
}

/// Type alias for results that may contain a traversal [`struct@Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// Returns the filesystem path at which this error occurred.
    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the traversal depth at which this error occurred.
    #[inline]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Returns the kind of this error.
    #[inline]
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Returns `true` if this error wraps an [`io::Error`].
    #[inline]
    pub fn is_io(&self) -> bool {
        matches!(self.kind, ErrorKind::Io(_))
    }

    /// Returns `true` if this error is a symlink loop detection.
    #[inline]
    pub fn is_loop(&self) -> bool {
        matches!(self.kind, ErrorKind::LoopDetected)
    }

    /// Returns the inner [`io::Error`], if this is an I/O error.
    #[inline]
    pub fn io_error(&self) -> Option<&io::Error> {
        match &self.kind {
            ErrorKind::Io(e) => Some(e),
            ErrorKind::LoopDetected => None,
        }
    }

    /// Consumes `self` and returns the inner [`io::Error`], if any.
    #[inline]
    pub fn into_io_error(self) -> Option<io::Error> {
        match self.kind {
            ErrorKind::Io(err) => Some(err),
            ErrorKind::LoopDetected => None,
        }
    }

    pub(crate) fn from_entry<T>(entry: &DirEntry<T>, source: io::Error) -> Self {
        Self {
            path: entry.path().to_path_buf(),
            depth: entry.depth(),
            kind: ErrorKind::Io(source),
        }
    }

    pub(crate) fn new_io_error(path: PathBuf, depth: usize, source: io::Error) -> Self {
        Self {
            path,
            depth,
            kind: ErrorKind::Io(source),
        }
    }

    pub(crate) fn loop_detected(path: PathBuf, depth: usize) -> Self {
        Self {
            path,
            depth,
            kind: ErrorKind::LoopDetected,
        }
    }
}
