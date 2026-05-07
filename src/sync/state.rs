use std::{cmp::Ordering, fmt, fs, path::PathBuf, vec};

use crate::{Ancestor, DirEntry, Error, Result, WalkDir, iter::WalkDirOptions};

/// Synchronous state.
#[derive(Debug)]
pub struct Sync;

struct LiveDirIter {
    rd: fs::ReadDir,
    path: PathBuf,
    depth: usize,
    follow_links: bool,
}

impl Iterator for LiveDirIter {
    type Item = Result<DirEntry<Sync>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.rd.next()? {
            Ok(raw) => Some(DirEntry::<Sync>::from_std(
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
    Sorted(vec::IntoIter<Result<DirEntry<Sync>>>),
}

impl Iterator for DirStream {
    type Item = Result<DirEntry<Sync>>;

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
    opts: WalkDirOptions<Sync>,
    ancestors: Vec<Ancestor>,
    stack: Vec<DirStream>,
    start: Option<Result<DirEntry<Sync>>>,
}

impl IntoIterator for WalkDir {
    type IntoIter = Walker;
    type Item = Result<DirEntry<Sync>>;

    fn into_iter(self) -> Self::IntoIter {
        let start = DirEntry::<Sync>::from_path(self.root, 0, self.opts.follow_links);
        Walker {
            start: Some(start),
            stack: vec![],
            ancestors: vec![],
            opts: self.opts,
        }
    }
}

impl Walker {
    fn push_dir(&mut self, entry: &DirEntry<Sync>) -> Result<()> {
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
        let child_depth = depth + 1;
        let follow_links = self.opts.follow_links;

        if self.opts.sort_by.is_some() || self.opts.group_dir {
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

    fn collect_sorted(
        &mut self,
        path: &std::path::Path,
        depth: usize,
    ) -> Result<Vec<Result<DirEntry<Sync>>>> {
        let follow_links = self.opts.follow_links;

        let rd = fs::read_dir(path)
            .map_err(|err| Error::new_io_error(path.to_path_buf(), depth, err))?;

        let mut entries: Vec<Result<DirEntry<Sync>>> = rd
            .map(|res| {
                res.map_err(|err| Error::new_io_error(path.to_path_buf(), depth, err))
                    .and_then(|raw| DirEntry::<Sync>::from_std(&raw, depth, follow_links))
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
    type Item = Result<DirEntry<Sync>>;

    fn next(&mut self) -> Option<Self::Item> {
        // Yield the root entry on the first call, descending into it if it is
        // a directory and within the depth limit.
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
