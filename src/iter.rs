use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

use crate::{DirEntry, Sync};

#[allow(clippy::type_complexity)]
pub(super) struct Sorter<T>(
    pub(super) Box<dyn FnMut(&DirEntry<T>, &DirEntry<T>) -> Ordering + Send + 'static>,
);

impl<T> Sorter<T> {
    pub(super) fn cmp(&mut self, a: &DirEntry<T>, b: &DirEntry<T>) -> Ordering {
        (self.0)(a, b)
    }
}

impl<T> std::fmt::Debug for Sorter<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sorter")
    }
}

#[derive(Debug)]
pub(super) struct WalkDirOptions<T> {
    pub(super) min_depth: usize,
    pub(super) max_depth: usize,
    pub(super) follow_links: bool,
    pub(super) group_dir: bool,
    pub(super) skip_hidden: bool,
    pub(super) sort_by: Option<Sorter<T>>,
}

impl<T> Default for WalkDirOptions<T> {
    fn default() -> Self {
        Self {
            min_depth: 0,
            max_depth: usize::MAX,
            follow_links: false,
            group_dir: false,
            skip_hidden: false,
            sort_by: None,
        }
    }
}

/// Builder for configuring a synchronous directory traversal.
#[derive(Debug)]
pub struct WalkDir {
    pub(super) root: PathBuf,
    pub(super) opts: WalkDirOptions<Sync>,
}

impl WalkDir {
    /// Creates a new traversal rooted at `root`.
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            opts: WalkDirOptions::default(),
        }
    }

    /// Sets the minimum traversal depth.
    pub fn min_depth(mut self, depth: usize) -> Self {
        self.opts.min_depth = depth;
        self
    }

    /// Sets the maximum traversal depth.
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.opts.max_depth = depth;
        self
    }

    /// Controls whether symbolic links are followed.
    pub fn follow_links(mut self, yes: bool) -> Self {
        self.opts.follow_links = yes;
        self
    }

    /// Controls whether directories are grouped before other entries.
    pub fn group_dir(mut self, yes: bool) -> Self {
        self.opts.group_dir = yes;
        self
    }

    /// Controls whether hidden entries are skipped.
    pub fn skip_hidden(mut self, yes: bool) -> Self {
        self.opts.skip_hidden = yes;
        self
    }

    /// Sets the comparison function used to sort entries within each directory.
    pub fn sort_by<F>(mut self, cmp: F) -> Self
    where
        F: FnMut(&DirEntry, &DirEntry) -> Ordering + Send + 'static,
    {
        self.opts.sort_by = Some(Sorter(Box::new(cmp)));
        self
    }
}
