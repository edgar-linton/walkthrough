use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

use crate::DirEntry;

/// Orders two entries within a directory.
///
/// Synchronous, and deliberately so: everything a [`DirEntry`] carries without
/// I/O — path, file name, file type, depth — is reachable from a comparator, and
/// [`DirEntry::metadata`] is cached, so reading it from here costs one `stat` per
/// entry rather than one per comparison. The async walker cannot borrow this
/// shape and takes a key function instead; see
/// [`AsyncWalkDir::sort_by`](crate::AsyncWalkDir::sort_by).
#[allow(clippy::type_complexity)]
pub(crate) struct Sorter(
    pub(crate) Box<dyn FnMut(&DirEntry, &DirEntry) -> Ordering + Send + 'static>,
);

impl Sorter {
    pub(crate) fn cmp(&mut self, a: &DirEntry, b: &DirEntry) -> Ordering {
        (self.0)(a, b)
    }
}

impl std::fmt::Debug for Sorter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sorter")
    }
}

// Configuration shared by both walkers. Sorting is not part of it: the two
// walkers order entries by different means, so each builder owns its own sorter.
#[derive(Debug)]
pub(crate) struct WalkDirOptions {
    pub(crate) min_depth: usize,
    pub(crate) max_depth: usize,
    pub(crate) follow_links: bool,
    pub(crate) group_dir: bool,
    pub(crate) skip_hidden: bool,
}

impl Default for WalkDirOptions {
    fn default() -> Self {
        Self {
            min_depth: 0,
            max_depth: usize::MAX,
            follow_links: false,
            group_dir: false,
            skip_hidden: false,
        }
    }
}

/// Builder for configuring a synchronous directory traversal.
#[derive(Debug)]
pub struct WalkDir {
    pub(crate) root: PathBuf,
    pub(crate) opts: WalkDirOptions,
    pub(crate) sort_by: Option<Sorter>,
}

impl WalkDir {
    /// Creates a new traversal rooted at `root`.
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            opts: WalkDirOptions::default(),
            sort_by: None,
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
    ///
    /// Applies only to entries discovered during recursion; a hidden
    /// directory is excluded from the walk entirely, not merely from the
    /// output, so its subtree is never read. The root passed to
    /// [`new`](Self::new) is exempt and is always yielded and descended into
    /// regardless of its own hidden status, since walking it was explicit.
    pub fn skip_hidden(mut self, yes: bool) -> Self {
        self.opts.skip_hidden = yes;
        self
    }

    /// Sets the comparator used to order the entries within each directory.
    ///
    /// The comparator may read anything a [`DirEntry`] offers, including
    /// [`metadata`](DirEntry::metadata) — the value is cached on the entry, so an
    /// ordering by size or timestamp costs one `stat` per entry, not one per
    /// comparison. Entries that failed outright are placed ahead of the rest
    /// without reaching the comparator, and [`group_dir`](Self::group_dir) is
    /// applied afterwards. Calling this more than once keeps the last comparator.
    ///
    /// The asynchronous counterpart takes a key function rather than a
    /// comparator, because a synchronous comparator cannot await
    /// [`AsyncDirEntry::metadata`](crate::AsyncDirEntry::metadata); see
    /// [`AsyncWalkDir::sort_by`](crate::AsyncWalkDir::sort_by).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use walkthrough::WalkDir;
    ///
    /// let walker = WalkDir::new(".").sort_by(|a, b| a.file_name().cmp(b.file_name()));
    /// ```
    pub fn sort_by<F>(mut self, cmp: F) -> Self
    where
        F: FnMut(&DirEntry, &DirEntry) -> Ordering + Send + 'static,
    {
        self.sort_by = Some(Sorter(Box::new(cmp)));
        self
    }
}
