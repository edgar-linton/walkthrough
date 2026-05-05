#![allow(unused)]
use std::{io, path::PathBuf};

use thiserror::Error;

use crate::DirEntry;

/// Errors that can occur during traversion.
#[derive(Debug, Error)]
#[error("Error pn path {path}, depth: {depth}: {kind}")]
pub struct Error {
    path: PathBuf,
    depth: usize,
    kind: ErrorKind,
}

impl Error {
    pub(crate) fn from_entry(entry: &DirEntry, error: io::Error) -> Self {
        Self::new_io_error(entry.path().to_path_buf(), entry.depth(), error)
    }

    pub(crate) fn new_io_error(path: PathBuf, depth: usize, error: io::Error) -> Self {
        Self::new(path, depth, ErrorKind::Io(error))
    }

    pub(crate) fn new_loop(path: PathBuf, depth: usize) -> Self {
        Self::new(path, depth, ErrorKind::LoopDetected)
    }

    fn new(path: PathBuf, depth: usize, kind: ErrorKind) -> Self {
        Self { path, depth, kind }
    }
}

#[derive(Debug, Error)]
enum ErrorKind {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("Loop detected.")]
    LoopDetected,
}

/// Type alias for an error.
pub type Result<T, E = Error> = std::result::Result<T, E>;
