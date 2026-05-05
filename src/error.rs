use std::{
    io,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::DirEntry;

/// Distinguishes the cause of a traversal [`Error`].
#[derive(Debug, Error)]
pub enum ErrorKind {
    /// An I/O error occurred while reading a directory entry or its metadata.
    #[error("{0}")]
    Io(#[source] io::Error),

    /// Following a symlink would revisit a directory that is already an
    /// ancestor in the current traversal path.
    #[error("symlink loop detected")]
    LoopDetected,
}

/// Error produced during directory traversal.
///
/// Always carries the filesystem [`path`](Self::path) and traversal
/// [`depth`](Self::depth) at which the failure occurred, together with a
/// [`kind`](Self::kind) that distinguishes the underlying cause.
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
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the traversal depth at which this error occurred.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Returns the kind of this error.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Returns `true` if this error wraps an [`io::Error`].
    pub fn is_io(&self) -> bool {
        matches!(self.kind, ErrorKind::Io(_))
    }

    /// Returns `true` if this error is a symlink loop detection.
    pub fn is_loop(&self) -> bool {
        matches!(self.kind, ErrorKind::LoopDetected)
    }

    /// Returns a reference to the inner [`io::Error`], or `None` if this is
    /// not an I/O error.
    pub fn io_error(&self) -> Option<&io::Error> {
        match &self.kind {
            ErrorKind::Io(e) => Some(e),
            ErrorKind::LoopDetected => None,
        }
    }

    /// Consumes `self` and returns the inner [`io::Error`], or `None` if this
    /// is not an I/O error.
    pub fn into_io_error(self) -> Option<io::Error> {
        match self.kind {
            ErrorKind::Io(e) => Some(e),
            ErrorKind::LoopDetected => None,
        }
    }

    pub(crate) fn from_entry(entry: &DirEntry, source: io::Error) -> Self {
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
