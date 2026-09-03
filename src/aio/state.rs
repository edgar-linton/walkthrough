use std::{
    fmt,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    vec,
};

use tokio::fs;

use super::{
    concurrent::{Candidate, Order, Raw, ResolveOpts, read_raw, resolve, sort_grouped},
    iter::{AsyncOptions, AsyncWalkDir, Filter, Sorter},
};
use crate::{DirEntry, Error, Result, options::WalkDirOptions};

/// Async state marker.
#[derive(Debug)]
pub struct Async;

// Stack item: pre-ordered entries (sort_by or group_dir set), or a live
// ReadDir handle, which never materialises the directory.
enum DirStream {
    Sorted(vec::IntoIter<Candidate>),
    Live {
        rd: Box<fs::ReadDir>,
        path: PathBuf,
        opts: ResolveOpts,
        // Resolved, not yet yielded; at most `concurrency` entries.
        ready: vec::IntoIter<Candidate>,
    },
}

impl fmt::Debug for DirStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirStream::Sorted(_) => f.write_str("Sorted(..)"),
            DirStream::Live { path, opts, .. } => f
                .debug_struct("Live")
                .field("path", path)
                .field("depth", &opts.depth)
                .finish_non_exhaustive(),
        }
    }
}

impl DirStream {
    async fn next_entry(&mut self) -> Option<Candidate> {
        match self {
            DirStream::Sorted(iter) => iter.next(),
            DirStream::Live {
                rd,
                path,
                opts,
                ready,
            } => loop {
                if let Some(candidate) = ready.next() {
                    return Some(candidate);
                }

                // Refill one batch, so its per-entry I/O overlaps.
                let mut raw = Vec::with_capacity(opts.concurrency);
                while raw.len() < opts.concurrency {
                    match rd.next_entry().await {
                        Ok(Some(entry)) => raw.push(Raw::Entry(entry)),
                        Ok(None) => break,
                        Err(err) => {
                            raw.push(Raw::Failed(Error::new_io_error(
                                path.clone(),
                                opts.depth,
                                err,
                            )));
                            break;
                        }
                    }
                }

                if raw.is_empty() {
                    return None;
                }

                *ready = resolve(raw, opts).await.into_iter();
                // `skip_hidden` and the filter can empty a whole batch, hence
                // the loop.
            },
        }
    }
}

impl AsyncWalkDir {
    /// Returns an async walker.
    ///
    /// An unreadable root arrives as the walker's first [`Err`].
    pub fn walker(self) -> AsyncWalker {
        let opts = self.opts;
        let async_opts = self.async_opts;
        AsyncWalker {
            opts,
            async_opts,
            stream: Box::pin(async_stream::stream! {
                let start =
                    DirEntry::<Async>::from_path(self.root, 0, self.opts.follow_links).await;
                let mut state = WalkerState {
                    start: Some(start),
                    stack: vec![],
                    ancestors: vec![],
                    #[cfg(windows)]
                    reparse_depth: None,
                    opts: self.opts,
                    async_opts: self.async_opts,
                    sort_by: self.sort_by,
                    filter: self.filter,
                    yielded: 0,
                    skipped: 0,
                };
                while let Some(entry) = state.next_entry().await {
                    yield entry;
                }
            }),
        }
    }
}

/// Async stateful walker produced by [`AsyncWalkDir::walker`].
pub struct AsyncWalker {
    opts: WalkDirOptions,
    async_opts: AsyncOptions,
    stream: Pin<Box<dyn futures_core::Stream<Item = Result<DirEntry<Async>>> + Send>>,
}

impl fmt::Debug for AsyncWalker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncWalker")
            .field("opts", &self.opts)
            .field("async_opts", &self.async_opts)
            .finish_non_exhaustive()
    }
}

impl AsyncWalker {
    /// Returns the next entry.
    pub async fn next_entry(&mut self) -> Option<Result<DirEntry<Async>>> {
        std::future::poll_fn(|cx| self.stream.as_mut().poll_next(cx)).await
    }
}

impl futures_core::Stream for AsyncWalker {
    type Item = Result<DirEntry<Async>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().stream.as_mut().poll_next(cx)
    }
}

/// The traversal state that [`AsyncWalker`]'s stream owns and drives.
#[derive(Debug)]
struct WalkerState {
    opts: WalkDirOptions,
    async_opts: AsyncOptions,
    sort_by: Option<Sorter>,
    filter: Option<Arc<Filter>>,
    ancestors: Vec<crate::Ancestor>,
    #[cfg(windows)]
    reparse_depth: Option<usize>,
    stack: Vec<DirStream>,
    start: Option<Result<DirEntry<Async>>>,
    yielded: usize,
    skipped: usize,
}

impl WalkerState {
    fn order(&self) -> Order {
        Order {
            group_dir: self.opts.group_dir,
            concurrency: self.async_opts.concurrency,
        }
    }

    async fn push_dir(&mut self, entry: &DirEntry<Async>) -> Result<()> {
        let depth = entry.depth();

        if self.track_ancestor(entry).await {
            self.ancestors.truncate(depth);

            if let Some(ancestor) = entry.ancestor().await {
                if self.ancestors.iter().any(|a| a == &ancestor) {
                    return Err(Error::loop_detected(entry.path().to_path_buf(), depth));
                }
                self.ancestors.push(ancestor);
            }
        }

        let path = entry.path().to_path_buf();
        let child_depth = depth + 1;
        let follow_links = self.opts.follow_links;

        let resolve_opts = ResolveOpts {
            depth: child_depth,
            follow_links,
            skip_hidden: self.opts.skip_hidden,
            concurrency: self.async_opts.concurrency,
            filter: self.filter.clone(),
        };

        if self.sort_by.is_some() || self.opts.group_dir {
            let entries = self.collect_ordered(&path, &resolve_opts).await?;
            self.stack.push(DirStream::Sorted(entries.into_iter()));
        } else {
            let rd = fs::read_dir(&path)
                .await
                .map_err(|err| Error::new_io_error(path.clone(), child_depth, err))?;
            self.stack.push(DirStream::Live {
                rd: Box::new(rd),
                path,
                opts: resolve_opts,
                ready: Vec::new().into_iter(),
            });
        }
        Ok(())
    }

    /// Whether a cycle through `entry` is reachable, and its identity
    /// therefore worth the `stat` it costs.
    ///
    /// Reaching an already-visited directory on Unix means traversing a
    /// symlink, so with links unfollowed no cycle exists to find.
    #[cfg(unix)]
    async fn track_ancestor(&mut self, _entry: &DirEntry<Async>) -> bool {
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
    async fn track_ancestor(&mut self, entry: &DirEntry<Async>) -> bool {
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
            self.backfill_ancestors(entry.path(), depth).await;
        }
        true
    }

    /// Records identity for the levels above `path`, once per reparse point
    /// that arms detection.
    ///
    /// Nothing above the first one was recorded, and a cycle is compared
    /// against the whole path. `O(depth)`.
    #[cfg(windows)]
    async fn backfill_ancestors(&mut self, path: &Path, depth: usize) {
        // Anything still held belongs to a subtree the walk has left.
        self.ancestors.clear();
        // The walk builds every path by joining one component per level, so
        // the levels above `path` are exactly its ancestors, deepest first.
        let mut above: Vec<&Path> = path.ancestors().skip(1).take(depth).collect();
        above.reverse();
        for level in above {
            if let Some(ancestor) = super::windows::ancestor_of(level).await {
                self.ancestors.push(ancestor);
            }
        }
    }

    async fn collect_ordered(
        &self,
        path: &Path,
        resolve_opts: &ResolveOpts,
    ) -> Result<Vec<Candidate>> {
        let order = self.order();
        let raw = read_raw(path, resolve_opts.depth).await?;
        let mut entries = resolve(raw, resolve_opts).await;

        match self.sort_by {
            Some(ref sorter) => entries = sorter.sort(entries, order).await,
            None => sort_grouped(&mut entries, order),
        }

        Ok(entries)
    }

    /// The paginated traversal: `skip(offset).take(limit)`.
    async fn next_entry(&mut self) -> Option<Result<DirEntry<Async>>> {
        loop {
            if self.yielded >= self.async_opts.limit {
                return None;
            }

            let entry = self.next_unpaged().await?;

            if self.skipped < self.async_opts.offset {
                self.skipped += 1;
                continue;
            }

            self.yielded += 1;
            return Some(entry);
        }
    }

    /// The traversal itself, without pagination.
    async fn next_unpaged(&mut self) -> Option<Result<DirEntry<Async>>> {
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
            let candidate = {
                let stream = self.stack.last_mut()?;
                match stream.next_entry().await {
                    Some(candidate) => candidate,
                    None => {
                        self.stack.pop();
                        continue;
                    }
                }
            };

            // `skip_hidden` and `IgnoreDir` are applied during resolve; neither
            // reaches here.
            let Candidate { yielded, entry } = candidate;
            let entry = match entry {
                Err(err) => return Some(Err(err)),
                Ok(e) => e,
            };

            if entry.is_dir()
                && entry.depth() < self.opts.max_depth
                && let Err(err) = self.push_dir(&entry).await
            {
                return Some(Err(err));
            }

            if yielded && entry.depth() >= self.opts.min_depth {
                return Some(Ok(entry));
            }
        }
    }
}
