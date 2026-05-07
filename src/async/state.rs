use std::{
    cmp::Ordering,
    fmt,
    path::{Path, PathBuf},
    vec,
};

use tokio::fs;

use crate::{DirEntry, Error, Result, iter::WalkDirOptions};

/// Async state marker.
#[derive(Debug)]
pub struct Async;

// Stack item: pre-sorted entries (when sort_by or group_dir is set) or a live
// ReadDir handle (the common unsorted case, which avoids collecting upfront).
enum DirStream {
    Sorted(vec::IntoIter<Result<DirEntry<Async>>>),
    Live {
        rd: Box<fs::ReadDir>,
        path: PathBuf,
        depth: usize,
        follow_links: bool,
    },
}

impl fmt::Debug for DirStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirStream::Sorted(_) => f.write_str("Sorted(..)"),
            DirStream::Live { path, depth, .. } => f
                .debug_struct("Live")
                .field("path", path)
                .field("depth", depth)
                .finish_non_exhaustive(),
        }
    }
}

impl DirStream {
    async fn next_entry(&mut self) -> Option<Result<DirEntry<Async>>> {
        match self {
            DirStream::Sorted(iter) => iter.next(),
            DirStream::Live {
                rd,
                path,
                depth,
                follow_links,
            } => match rd.next_entry().await {
                Ok(Some(raw)) => {
                    Some(DirEntry::<Async>::from_std(&raw, *depth, *follow_links).await)
                }
                Ok(None) => None,
                Err(err) => Some(Err(Error::new_io_error(path.clone(), *depth, err))),
            },
        }
    }
}

/// Builder for configuring an asynchronous directory traversal.
#[derive(Debug)]
pub struct AsyncWalkDir {
    root: PathBuf,
    opts: WalkDirOptions<Async>,
}

impl AsyncWalkDir {
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
        F: FnMut(&DirEntry<Async>, &DirEntry<Async>) -> Ordering + Send + 'static,
    {
        self.opts.sort_by = Some(crate::iter::Sorter(Box::new(cmp)));
        self
    }

    /// Returns an async walker.
    pub async fn walker(self) -> AsyncWalker {
        let start = DirEntry::<Async>::from_path(self.root, 0, self.opts.follow_links).await;
        AsyncWalker {
            start: Some(start),
            stack: vec![],
            ancestors: vec![],
            opts: self.opts,
        }
    }
}

/// Async stateful walker produced by [`AsyncWalkDir::walker`].
#[derive(Debug)]
pub struct AsyncWalker {
    opts: WalkDirOptions<Async>,
    ancestors: Vec<crate::Ancestor>,
    stack: Vec<DirStream>,
    start: Option<Result<DirEntry<Async>>>,
}

impl AsyncWalker {
    async fn push_dir(&mut self, entry: &DirEntry<Async>) -> Result<()> {
        let depth = entry.depth();
        self.ancestors.truncate(depth);

        if let Some(ancestor) = entry.ancestor().await {
            if self.ancestors.iter().any(|a| a == &ancestor) {
                return Err(Error::loop_detected(entry.path().to_path_buf(), depth));
            }
            self.ancestors.push(ancestor);
        }

        let path = entry.path().to_path_buf();
        let child_depth = depth + 1;
        let follow_links = self.opts.follow_links;

        if self.opts.sort_by.is_some() || self.opts.group_dir {
            let entries = self.collect_sorted(&path, child_depth).await?;
            self.stack.push(DirStream::Sorted(entries.into_iter()));
        } else {
            let rd = fs::read_dir(&path)
                .await
                .map_err(|err| Error::new_io_error(path.clone(), child_depth, err))?;
            self.stack.push(DirStream::Live {
                rd: Box::new(rd),
                path,
                depth: child_depth,
                follow_links,
            });
        }
        Ok(())
    }

    async fn collect_sorted(
        &mut self,
        path: &std::path::Path,
        depth: usize,
    ) -> Result<Vec<Result<DirEntry<Async>>>> {
        let follow_links = self.opts.follow_links;

        let mut rd = fs::read_dir(path)
            .await
            .map_err(|err| Error::new_io_error(path.to_path_buf(), depth, err))?;

        let mut entries: Vec<Result<DirEntry<Async>>> = Vec::new();
        loop {
            match rd.next_entry().await {
                Ok(Some(raw)) => {
                    entries.push(DirEntry::<Async>::from_std(&raw, depth, follow_links).await);
                }
                Ok(None) => break,
                Err(err) => entries.push(Err(Error::new_io_error(path.to_path_buf(), depth, err))),
            }
        }

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

    /// Returns the next entry.
    pub async fn next(&mut self) -> Option<Result<DirEntry<Async>>> {
        if let Some(res) = self.start.take() {
            let entry = match res {
                Err(err) => return Some(Err(err)),
                Ok(e) => e,
            };
            if entry.is_dir()
                && entry.depth() < self.opts.max_depth
                && let Err(err) = self.push_dir(&entry).await
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
                match stream.next_entry().await {
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

            if self.opts.skip_hidden && entry.is_hidden().await {
                continue;
            }

            if entry.is_dir()
                && entry.depth() < self.opts.max_depth
                && let Err(err) = self.push_dir(&entry).await
            {
                return Some(Err(err));
            }

            if entry.depth() >= self.opts.min_depth {
                return Some(Ok(entry));
            }
        }
    }
}
