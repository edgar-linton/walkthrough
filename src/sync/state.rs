use std::{
    cmp::Ordering,
    fmt, fs,
    path::{Path, PathBuf},
    vec,
};

use crate::{
    Ancestor, DirEntry, Error, Result, WalkDir,
    iter::{Sorter, WalkDirOptions},
};

/// Blocking state marker.
#[derive(Debug)]
pub struct Blocking;

struct LiveDirIter {
    rd: fs::ReadDir,
    path: PathBuf,
    depth: usize,
    follow_links: bool,
}

impl Iterator for LiveDirIter {
    type Item = Result<DirEntry<Blocking>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.rd.next()? {
            Ok(raw) => Some(DirEntry::<Blocking>::from_std(
                &raw,
                self.depth,
                self.follow_links,
            )),
            Err(err) => Some(Err(Error::new_io_error(self.path.clone(), self.depth, err))),
        }
    }
}

enum DirStream {
    Live(Box<LiveDirIter>),
    Sorted(vec::IntoIter<Result<DirEntry<Blocking>>>),
}

impl Iterator for DirStream {
    type Item = Result<DirEntry<Blocking>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            DirStream::Live(iter) => iter.next(),
            DirStream::Sorted(iter) => iter.next(),
        }
    }
}

impl fmt::Debug for DirStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirStream::Live(_) => f.write_str("DirStream::Live"),
            DirStream::Sorted(_) => f.write_str("DirStream::Sorted"),
        }
    }
}

/// Stateful iterator produced by [`WalkDir`].
#[derive(Debug)]
pub struct Walker {
    opts: WalkDirOptions,
    sort_by: Option<Sorter>,
    ancestors: Vec<Ancestor>,
    #[cfg(windows)]
    reparse_depth: Option<usize>,
    stack: Vec<DirStream>,
    start: Option<Result<DirEntry<Blocking>>>,
}

impl IntoIterator for WalkDir {
    type IntoIter = Walker;
    type Item = Result<DirEntry<Blocking>>;

    fn into_iter(self) -> Self::IntoIter {
        let start = DirEntry::<Blocking>::from_path(self.root, 0, self.opts.follow_links);
        Walker {
            start: Some(start),
            stack: vec![],
            ancestors: vec![],
            #[cfg(windows)]
            reparse_depth: None,
            opts: self.opts,
            sort_by: self.sort_by,
        }
    }
}

impl Walker {
    fn push_dir(&mut self, entry: &DirEntry<Blocking>) -> Result<()> {
        let depth = entry.depth();

        if self.track_ancestor(entry) {
            // Truncating evicts a previous subtree's ancestors, so backtracking
            // needs no explicit pops.
            self.ancestors.truncate(depth);

            if let Some(ancestor) = entry.ancestor() {
                if self.ancestors.iter().any(|a| a == &ancestor) {
                    return Err(Error::loop_detected(entry.path().to_path_buf(), depth));
                }
                self.ancestors.push(ancestor);
            }
        }

        let path = entry.path().to_path_buf();
        let child_depth = depth + 1;
        let follow_links = self.opts.follow_links;

        if self.sort_by.is_some() || self.opts.group_dir {
            let entries = self.collect_sorted(&path, child_depth)?;
            self.stack.push(DirStream::Sorted(entries.into_iter()));
        } else {
            let rd = fs::read_dir(&path)
                .map_err(|err| Error::new_io_error(path.clone(), child_depth, err))?;
            self.stack.push(DirStream::Live(Box::new(LiveDirIter {
                rd,
                path,
                depth: child_depth,
                follow_links,
            })));
        }
        Ok(())
    }

    /// Whether a cycle through `entry` is reachable, and its identity
    /// therefore worth the `stat` it costs.
    ///
    /// Reaching an already-visited directory on Unix means traversing a
    /// symlink, so with links unfollowed no cycle exists to find.
    #[cfg(unix)]
    fn track_ancestor(&mut self, _entry: &DirEntry<Blocking>) -> bool {
        self.opts.follow_links
    }

    /// Whether a cycle through `entry` is reachable, and its identity
    /// therefore worth the handle open it costs.
    ///
    /// Unfollowed links do not rule a cycle out here the way they do on Unix,
    /// because a directory junction can be reported as a plain directory. A
    /// reparse point somewhere on the path is what a cycle does need, and the
    /// directory scan already says which entries are one, so identity — the
    /// most expensive per-directory operation on Windows — waits for the first
    /// of them and backfills the levels above it. A tree holding no reparse
    /// point at all, which is the overwhelming majority, opens no handle.
    #[cfg(windows)]
    fn track_ancestor(&mut self, entry: &DirEntry<Blocking>) -> bool {
        if self.opts.follow_links {
            return true;
        }
        let depth = entry.depth();
        // A reparse point recorded at this depth or deeper sits in a subtree
        // the walk has left, so it is no longer on the path.
        if self.reparse_depth.is_some_and(|d| d >= depth) {
            self.reparse_depth = None;
        }
        if self.reparse_depth.is_none() {
            if !entry.is_reparse_point() {
                return false;
            }
            self.reparse_depth = Some(depth);
            self.backfill_ancestors(entry.path(), depth);
        }
        true
    }

    /// Records identity for the levels above `path`, once per reparse point
    /// that arms detection.
    ///
    /// Nothing above the first one was recorded, and a cycle is compared
    /// against the whole path. `O(depth)`.
    #[cfg(windows)]
    fn backfill_ancestors(&mut self, path: &Path, depth: usize) {
        // Anything still held belongs to a subtree the walk has left.
        self.ancestors.clear();
        // The walk builds every path by joining one component per level, so
        // the levels above `path` are exactly its ancestors, deepest first.
        let mut above: Vec<&Path> = path.ancestors().skip(1).take(depth).collect();
        above.reverse();
        for level in above {
            if let Some(ancestor) = super::windows::ancestor_of(level) {
                self.ancestors.push(ancestor);
            }
        }
    }

    fn collect_sorted(
        &mut self,
        path: &Path,
        depth: usize,
    ) -> Result<Vec<Result<DirEntry<Blocking>>>> {
        let follow_links = self.opts.follow_links;

        let rd = fs::read_dir(path)
            .map_err(|err| Error::new_io_error(path.to_path_buf(), depth, err))?;

        let mut entries: Vec<Result<DirEntry<Blocking>>> = rd
            .map(|res| {
                res.map_err(|err| Error::new_io_error(path.to_path_buf(), depth, err))
                    .and_then(|raw| DirEntry::<Blocking>::from_std(&raw, depth, follow_links))
            })
            .collect();

        if let Some(ref mut sorter) = self.sort_by {
            entries.sort_by(|a, b| match (a, b) {
                (Ok(a), Ok(b)) => sorter.cmp(a, b),
                // Failed entries keep their place ahead of the rest.
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
    type Item = Result<DirEntry<Blocking>>;

    fn next(&mut self) -> Option<Self::Item> {
        // First call: yield the root, descending if it is a directory within
        // the depth limit.
        if let Some(res) = self.start.take() {
            let entry = match res {
                Err(err) => return Some(Err(err)),
                Ok(e) => e,
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
                let stream = self.stack.last_mut()?;
                match stream.next() {
                    Some(res) => res,
                    None => {
                        self.stack.pop();
                        continue;
                    }
                }
            };

            let entry = match res {
                Err(err) => return Some(Err(err)),
                Ok(e) => e,
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
