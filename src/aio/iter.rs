use std::{
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use super::{
    concurrent::{Candidate, Order, sort_by_key},
    state::Async,
};
use crate::{DirEntry, options::WalkDirOptions};

/// Default for [`AsyncWalkDir::concurrency`].
const DEFAULT_CONCURRENCY: usize = 32;

/// Configuration that only the asynchronous walker has.
#[derive(Debug, Clone, Copy)]
pub(super) struct AsyncOptions {
    pub(super) concurrency: usize,
    pub(super) limit: usize,
    pub(super) offset: usize,
}

impl Default for AsyncOptions {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_CONCURRENCY,
            limit: usize::MAX,
            offset: 0,
        }
    }
}

type SortFuture = Pin<Box<dyn Future<Output = Vec<Candidate>> + Send>>;

/// Orders the entries of one directory by an asynchronously computed key.
///
/// Holds the whole resolve-and-sort step, not the key function, so the walker
/// need not be generic over the key type. `Sync` because the walker borrows it
/// across an `await`.
#[allow(clippy::type_complexity)]
pub(super) struct Sorter(Box<dyn Fn(Vec<Candidate>, Order) -> SortFuture + Send + Sync>);

impl Sorter {
    pub(super) async fn sort(&self, entries: Vec<Candidate>, order: Order) -> Vec<Candidate> {
        (self.0)(entries, order).await
    }
}

impl fmt::Debug for Sorter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sorter")
    }
}

/// What [`AsyncWalkDir::filter_entry`] decides about one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filtering {
    /// Yield the entry, and descend into it if it is a directory.
    Continue,
    /// Do not yield the entry, but still descend into it if it is a directory.
    IgnoreEntry,
    /// Do not yield the entry, and do not descend into it.
    IgnoreDir,
}

type FilterFuture = Pin<Box<dyn Future<Output = Filtering> + Send>>;

/// Decides each entry's fate before the walker can descend into it.
///
/// `Sync` because the walker shares one filter across the tasks resolving a
/// directory.
#[allow(clippy::type_complexity)]
pub(super) struct Filter(Box<dyn Fn(Arc<DirEntry<Async>>) -> FilterFuture + Send + Sync>);

impl Filter {
    /// Returns the verdict and the entry, which the predicate saw through an
    /// [`Arc`] so it could hold it across an `await`.
    pub(super) async fn decide(&self, entry: Arc<DirEntry<Async>>) -> (Filtering, DirEntry<Async>) {
        let verdict = (self.0)(Arc::clone(&entry)).await;
        (verdict, Arc::unwrap_or_clone(entry))
    }
}

impl fmt::Debug for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Filter")
    }
}

/// Builder for configuring an asynchronous directory traversal.
#[derive(Debug)]
pub struct AsyncWalkDir {
    pub(super) root: PathBuf,
    pub(super) opts: WalkDirOptions,
    pub(super) async_opts: AsyncOptions,
    pub(super) sort_by: Option<Sorter>,
    pub(super) filter: Option<Arc<Filter>>,
}

impl AsyncWalkDir {
    /// Creates a new traversal rooted at `root`.
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            opts: WalkDirOptions::default(),
            async_opts: AsyncOptions::default(),
            sort_by: None,
            filter: None,
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

    /// Sets how many entries of one directory may have I/O in flight at once.
    ///
    /// Defaults to 32, clamped to at least 1. Never affects order.
    pub fn concurrency(mut self, n: usize) -> Self {
        self.async_opts.concurrency = n.max(1);
        self
    }

    /// Yields at most `limit` entries, then ends the walk.
    ///
    /// Counted over the whole traversal, not per directory. Errors count as
    /// entries. `limit(0)` yields nothing.
    pub fn limit(mut self, limit: usize) -> Self {
        self.async_opts.limit = limit;
        self
    }

    /// Skips the first `offset` entries the walk would yield.
    ///
    /// A skipped directory is still descended into. An `offset` past the end
    /// yields nothing. Requires a reproducible order — set
    /// [`sort_by`](Self::sort_by) or [`group_dir`](Self::group_dir).
    pub fn offset(mut self, offset: usize) -> Self {
        self.async_opts.offset = offset;
        self
    }

    /// Sets the predicate deciding each entry's fate before descent.
    ///
    /// The predicate may await [`metadata`](DirEntry::metadata). The root is
    /// exempt, an entry that failed to resolve is reported without consulting
    /// it, and the last call wins.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn run() -> walkthrough::Result<()> {
    /// use walkthrough::{AsyncWalkDir, Filtering};
    ///
    /// let mut walker = AsyncWalkDir::new(".")
    ///     .filter_entry(|entry| async move {
    ///         if entry.file_name() == "target" {
    ///             Filtering::IgnoreDir
    ///         } else {
    ///             Filtering::Continue
    ///         }
    ///     })
    ///     .walker();
    ///
    /// while let Some(entry) = walker.next_entry().await {
    ///     println!("{:?}", entry?.path());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn filter_entry<Fut, F>(mut self, predicate: F) -> Self
    where
        F: Fn(Arc<DirEntry<Async>>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Filtering> + Send + 'static,
    {
        self.filter = Some(Arc::new(Filter(Box::new(move |entry| {
            Box::pin(predicate(entry))
        }))));
        self
    }

    /// Sets the key used to order the entries within each directory.
    ///
    /// Each key is awaited once before ordering. The order is total:
    /// [`group_dir`](Self::group_dir), then entries that failed outright, then
    /// the key, then the file name. The last call wins.
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
    ///     .walker();
    ///
    /// while let Some(entry) = walker.next_entry().await {
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
        self.sort_by = Some(Sorter(Box::new(move |entries, order| {
            Box::pin(sort_by_key(Arc::clone(&key), entries, order))
        })));
        self
    }
}
