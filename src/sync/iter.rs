use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

use crate::{DirEntry, options::WalkDirOptions};

/// Orders two entries within a directory.
///
/// [`DirEntry::metadata`] is cached, so reading it here costs one `stat` per
/// entry, not one per comparison.
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
    /// A hidden directory's subtree is never read. The root is exempt.
    pub fn skip_hidden(mut self, yes: bool) -> Self {
        self.opts.skip_hidden = yes;
        self
    }

    /// Sets the comparator used to order the entries within each directory.
    ///
    /// Failed entries sort first, without reaching the comparator.
    /// [`group_dir`](Self::group_dir) applies afterwards. The last call wins.
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
