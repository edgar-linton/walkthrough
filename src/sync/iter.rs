use std::{
    cmp::Ordering,
    fmt, fs,
    path::{Path, PathBuf},
    vec,
};

use super::Ancestor;
use crate::{DirEntry, Error, Result};

#[allow(clippy::type_complexity)]
struct Sorter(Box<dyn FnMut(&DirEntry, &DirEntry) -> Ordering + Send + 'static>);

impl Sorter {
    fn cmp(&mut self, a: &DirEntry, b: &DirEntry) -> Ordering {
        (self.0)(a, b)
    }
}

impl fmt::Debug for Sorter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sorter")
    }
}

#[derive(Debug)]
struct WalkDirOptions {
    min_depth: usize,
    max_depth: usize,
    follow_links: bool,
    group_dir: bool,
    skip_hidden: bool,
    sort_by: Option<Sorter>,
}

impl Default for WalkDirOptions {
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

/// Builder for configuring a directory traversal.
#[derive(Debug)]
pub struct WalkDir {
    root: PathBuf,
    opts: WalkDirOptions,
}

impl WalkDir {
    /// Creates a new traversal rooted at `root`.
    pub fn new<P>(root: P) -> Self
    where
        P: AsRef<Path>,
    {
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

impl IntoIterator for WalkDir {
    type IntoIter = Walker;
    type Item = Result<DirEntry>;

    fn into_iter(self) -> Self::IntoIter {
        let start = DirEntry::from_path(self.root, 0, self.opts.follow_links);
        Walker {
            start: Some(start),
            stack: vec![],
            ancestors: vec![],
            opts: self.opts,
        }
    }
}

/// Stateful iterator produced by [`WalkDir`].
#[derive(Debug)]
pub struct Walker {
    opts: WalkDirOptions,
    ancestors: Vec<Ancestor>,
    /// One sorted-entry iterator per directory level currently open.
    stack: Vec<vec::IntoIter<Result<DirEntry>>>,
    /// The root entry, yielded on the first call to `next`.
    start: Option<Result<DirEntry>>,
}

impl Walker {
    /// Descends into `entry`: checks for symlink loops, then reads and sorts
    /// its children, pushing the result onto the stack.
    fn push_dir(&mut self, entry: &DirEntry) -> Result<()> {
        let depth = entry.depth();
        // Truncating to `depth` evicts ancestors from any previous subtree at
        // this level — the correct way to handle backtracking without explicit pops.
        self.ancestors.truncate(depth);

        if let Some(ancestor) = entry.ancestor() {
            if self.ancestors.iter().any(|a| a == &ancestor) {
                return Err(Error::loop_detected(entry.path().to_path_buf(), depth));
            }
            self.ancestors.push(ancestor);
        }

        let path = entry.path().to_path_buf();
        let entries = self.read_dir(&path, depth + 1)?;
        self.stack.push(entries.into_iter());
        Ok(())
    }

    /// Reads a directory at `path`, returning its entries sorted according to
    /// the configured options (`sort_by`, then `group_dir`).
    fn read_dir(&mut self, path: &Path, depth: usize) -> Result<Vec<Result<DirEntry>>> {
        let follow_links = self.opts.follow_links;

        let rd = fs::read_dir(path)
            .map_err(|err| Error::new_io_error(path.to_path_buf(), depth, err))?;

        let mut entries: Vec<Result<DirEntry>> = rd
            .map(|res| {
                res.map_err(|err| Error::new_io_error(path.to_path_buf(), depth, err))
                    .and_then(|raw| DirEntry::from_std(&raw, depth, follow_links))
            })
            .collect();

        if let Some(ref mut sorter) = self.opts.sort_by {
            entries.sort_by(|a, b| match (a, b) {
                (Ok(a), Ok(b)) => sorter.cmp(a, b),
                (Err(_), Ok(_)) => Ordering::Less,
                (Ok(_), Err(_)) => Ordering::Greater,
                (Err(_), Err(_)) => Ordering::Equal,
            });
        }

        if self.opts.group_dir {
            entries.sort_by(|a, b| {
                let a_dir = a.as_ref().is_ok_and(DirEntry::is_dir);
                let b_dir = b.as_ref().is_ok_and(DirEntry::is_dir);
                b_dir.cmp(&a_dir)
            });
        }

        Ok(entries)
    }
}

impl Iterator for Walker {
    type Item = Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        // Yield the root entry on the first call, descending into it if it is
        // a directory and within the depth limit.
        if let Some(res) = self.start.take() {
            let entry = match res {
                Err(err) => return Some(Err(err)),
                Ok(err) => err,
            };
            if entry.is_dir()
                && entry.depth() < self.opts.max_depth
                && let Err(err) = self.push_dir(&entry)
            {
                return Some(Err(err));
            }
            if entry.depth() >= self.opts.min_depth {
                return Some(Ok(entry));
            }
        }

        loop {
            let res = {
                let iter = self.stack.last_mut()?;
                match iter.next() {
                    Some(res) => res,
                    None => {
                        self.stack.pop();
                        continue;
                    }
                }
            };

            let entry = match res {
                Err(err) => return Some(Err(err)),
                Ok(err) => err,
            };

            if self.opts.skip_hidden && entry.is_hidden() {
                continue;
            }

            if entry.is_dir()
                && entry.depth() < self.opts.max_depth
                && let Err(err) = self.push_dir(&entry)
            {
                return Some(Err(err));
            }

            if entry.depth() >= self.opts.min_depth {
                return Some(Ok(entry));
            }
        }
    }
}
