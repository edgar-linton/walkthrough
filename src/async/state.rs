use std::{
    cmp::Ordering,
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    vec,
};

use tokio::fs;

use crate::{DirEntry, Error, Result, iter::WalkDirOptions};

/// Async state marker.
#[derive(Debug)]
pub struct Async;

type SortFuture = Pin<Box<dyn Future<Output = Vec<Result<DirEntry<Async>>>> + Send>>;

/// Orders the entries of one directory by an asynchronously computed key.
///
/// Holds the whole decorate-sort-undecorate step rather than the key function,
/// which is why the key type does not appear here: [`AsyncWalkDir::sort_by`]
/// monomorphises the step and stores it as one closure, so the walker can order
/// entries by any [`Ord`] key without becoming generic over it.
///
/// `Sync` as well as `Send`, because the walker borrows the sorter across an
/// `await` and a walk has to stay spawnable.
#[allow(clippy::type_complexity)]
struct Sorter(Box<dyn Fn(Vec<Result<DirEntry<Async>>>) -> SortFuture + Send + Sync>);

impl Sorter {
    async fn sort(&self, entries: Vec<Result<DirEntry<Async>>>) -> Vec<Result<DirEntry<Async>>> {
        (self.0)(entries).await
    }
}

impl fmt::Debug for Sorter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sorter")
    }
}

/// Orders `entries` by `key`, awaiting the key of every entry exactly once.
///
/// The entry is shared with the key function through an [`Arc`] and recovered
/// afterwards, so metadata the key resolved stays cached on the entry the walker
/// goes on to yield.
async fn sort_by_key<K, Fut, F>(
    key: Arc<F>,
    entries: Vec<Result<DirEntry<Async>>>,
) -> Vec<Result<DirEntry<Async>>>
where
    F: Fn(Arc<DirEntry<Async>>) -> Fut,
    Fut: Future<Output = K>,
    K: Ord,
{
    let mut decorated: Vec<(Option<K>, Result<DirEntry<Async>>)> =
        Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            Ok(entry) => {
                let shared = Arc::new(entry);
                let k = key(Arc::clone(&shared)).await;
                decorated.push((Some(k), Ok(Arc::unwrap_or_clone(shared))));
            }
            // An entry that failed outright has no key; it sorts ahead of the rest.
            Err(err) => decorated.push((None, Err(err))),
        }
    }

    decorated.sort_by(|(a, _), (b, _)| match (a, b) {
        (Some(a), Some(b)) => a.cmp(b),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });

    decorated.into_iter().map(|(_, entry)| entry).collect()
}

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
    opts: WalkDirOptions,
    sort_by: Option<Sorter>,
}

impl AsyncWalkDir {
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

    /// Sets the key used to order the entries within each directory.
    ///
    /// The key is computed asynchronously, so it may await
    /// [`metadata`](DirEntry::metadata) — the one property of an entry that is
    /// not reachable without I/O. Ordering by size or modification time is
    /// therefore expressed here rather than through a comparator: a synchronous
    /// comparator cannot await, and blocking inside one panics with "Cannot start
    /// a runtime from within a runtime" on any Tokio runtime, which is a `500` if
    /// the walk happens inside a request handler.
    ///
    /// The walker awaits the key of every entry exactly once, before ordering, so
    /// an ordering by size costs one `stat` per entry rather than one per
    /// comparison. Keys that await nothing cost nothing beyond the key itself.
    ///
    /// The entry arrives as an [`Arc`] so the key may hold it across an `await`;
    /// metadata resolved inside the key stays cached on the entry the walk yields.
    /// Compound orderings are tuple keys, and descending order is
    /// [`Reverse`](std::cmp::Reverse). Entries that failed outright are placed
    /// ahead of the rest without a key being computed for them, and
    /// [`group_dir`](Self::group_dir) is applied afterwards, so it still wins over
    /// the key. Calling this more than once keeps the last key.
    ///
    /// The synchronous counterpart takes a comparator rather than a key, since
    /// [`DirEntry::metadata`](crate::DirEntry::metadata) is directly readable
    /// there; see [`WalkDir::sort_by`](crate::WalkDir::sort_by).
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn run() -> walkthrough::Result<()> {
    /// use walkthrough::AsyncWalkDir;
    ///
    /// // Directories first, then smallest file first, ties broken by name.
    /// let mut walker = AsyncWalkDir::new("src")
    ///     .sort_by(|entry| async move {
    ///         let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
    ///         (!entry.is_dir(), size, entry.file_name().to_owned())
    ///     })
    ///     .walker()
    ///     .await;
    ///
    /// while let Some(entry) = walker.next().await {
    ///     println!("{:?}", entry?.path());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn sort_by<K, Fut, F>(mut self, key: F) -> Self
    where
        F: Fn(Arc<DirEntry<Async>>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = K> + Send + 'static,
        K: Ord + Send + 'static,
    {
        let key = Arc::new(key);
        self.sort_by = Some(Sorter(Box::new(move |entries| {
            Box::pin(sort_by_key(Arc::clone(&key), entries))
        })));
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
            sort_by: self.sort_by,
        }
    }
}

/// Async stateful walker produced by [`AsyncWalkDir::walker`].
#[derive(Debug)]
pub struct AsyncWalker {
    opts: WalkDirOptions,
    sort_by: Option<Sorter>,
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

        if self.sort_by.is_some() || self.opts.group_dir {
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
        &self,
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

        if let Some(ref sorter) = self.sort_by {
            entries = sorter.sort(entries).await;
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

    /// Converts this walker into a [`futures_core::Stream`].
    pub fn into_stream(self) -> impl futures_core::Stream<Item = Result<DirEntry<Async>>> {
        async_stream::stream! {
            let mut walker = self;
            while let Some(entry) = walker.next().await {
                yield entry;
            }
        }
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
