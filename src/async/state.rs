use std::{
    ffi::OsString,
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    vec,
};

use tokio::{fs, task::JoinSet};

use crate::{DirEntry, Error, Result, iter::WalkDirOptions};

/// Async state marker.
#[derive(Debug)]
pub struct Async;

/// Default for [`AsyncWalkDir::concurrency`].
const DEFAULT_CONCURRENCY: usize = 32;

/// Whether ancestor bookkeeping may be skipped when
/// [`AsyncWalkDir::follow_links`] is off.
///
/// False on Windows: a directory junction can be reported as a directory
/// rather than a link, so a cycle through one stays reachable. A missed loop
/// walks forever.
#[cfg(unix)]
const LOOPS_NEED_FOLLOW_LINKS: bool = true;
#[cfg(windows)]
const LOOPS_NEED_FOLLOW_LINKS: bool = false;

/// Configuration that only the asynchronous walker has.
#[derive(Debug, Clone, Copy)]
struct AsyncOptions {
    concurrency: usize,
    limit: usize,
    offset: usize,
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

/// Everything ordering one directory needs beyond the entries themselves.
#[derive(Debug, Clone, Copy)]
struct Order {
    group_dir: bool,
    concurrency: usize,
}

impl Order {
    /// The grouping component of the order; `false` sorts first, so
    /// directories lead when [`group_dir`](AsyncWalkDir::group_dir) is set.
    fn grouped_entry(&self, entry: &DirEntry<Async>) -> bool {
        self.group_dir && !entry.is_dir()
    }

    /// As [`grouped_entry`](Self::grouped_entry), for a possibly failed entry,
    /// which is not a known directory.
    fn grouped(&self, entry: &Result<DirEntry<Async>>) -> bool {
        self.group_dir && !entry.as_ref().is_ok_and(DirEntry::is_dir)
    }
}

/// The name component of the order, which makes it total: a file name is
/// unique within its directory.
fn order_name(entry: &Result<DirEntry<Async>>) -> OsString {
    match entry {
        Ok(entry) => entry.file_name().to_owned(),
        Err(err) => error_name(err),
    }
}

/// Order name for a failed entry: the file name of the path it failed on.
fn error_name(err: &Error) -> OsString {
    err.path()
        .file_name()
        .unwrap_or_else(|| err.path().as_os_str())
        .to_owned()
}

/// One entry together with its position in its directory's total order.
struct Ordered<K> {
    grouped: bool,
    key: Option<K>,
    name: OsString,
    entry: Candidate,
}

/// One child of a directory, before its key is known.
enum Pending {
    /// Failed outright, so no key is computed for it.
    Failed(Error),
    Keyed {
        entry: Arc<DirEntry<Async>>,
        yielded: bool,
    },
}

type SortFuture = Pin<Box<dyn Future<Output = Vec<Candidate>> + Send>>;

/// Orders the entries of one directory by an asynchronously computed key.
///
/// Holds the whole resolve-and-sort step, not the key function, so the walker
/// need not be generic over the key type. `Sync` because the walker borrows it
/// across an `await`.
#[allow(clippy::type_complexity)]
struct Sorter(Box<dyn Fn(Vec<Candidate>, Order) -> SortFuture + Send + Sync>);

impl Sorter {
    async fn sort(&self, entries: Vec<Candidate>, order: Order) -> Vec<Candidate> {
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
struct Filter(Box<dyn Fn(Arc<DirEntry<Async>>) -> FilterFuture + Send + Sync>);

impl Filter {
    /// Returns the verdict and the entry, which the predicate saw through an
    /// [`Arc`] so it could hold it across an `await`.
    async fn decide(&self, entry: Arc<DirEntry<Async>>) -> (Filtering, DirEntry<Async>) {
        let verdict = (self.0)(Arc::clone(&entry)).await;
        (verdict, Arc::unwrap_or_clone(entry))
    }
}

impl fmt::Debug for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Filter")
    }
}

/// A resolved child together with whether the walk yields it.
struct Candidate {
    /// `false` for [`Filtering::IgnoreEntry`] on a directory: descended into,
    /// as an entry above `min_depth` is, but never yielded.
    yielded: bool,
    entry: Result<DirEntry<Async>>,
}

impl Candidate {
    fn kept(entry: Result<DirEntry<Async>>) -> Self {
        Self {
            yielded: true,
            entry,
        }
    }

    fn descend_only(entry: Result<DirEntry<Async>>) -> Self {
        Self {
            yielded: false,
            entry,
        }
    }
}

/// Everything resolving one directory's children needs.
#[derive(Debug, Clone)]
struct ResolveOpts {
    depth: usize,
    follow_links: bool,
    skip_hidden: bool,
    concurrency: usize,
    filter: Option<Arc<Filter>>,
}

/// Applies `task` to every item, at most `concurrency` in flight at once.
///
/// Results are placed by input position, never by completion order, so
/// concurrency cannot influence the order the walker yields.
async fn map_bounded<T, U, Fut, F>(items: Vec<T>, concurrency: usize, task: F) -> Vec<U>
where
    T: Send + 'static,
    U: Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = U> + Send + 'static,
{
    // Keeps `concurrency(1)` as cheap as a sequential walk.
    if concurrency <= 1 || items.len() <= 1 {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(task(item).await);
        }
        return out;
    }

    let task = Arc::new(task);
    let mut out: Vec<Option<U>> = items.iter().map(|_| None).collect();
    let mut queued = items.into_iter().enumerate();
    let mut set: JoinSet<(usize, U)> = JoinSet::new();

    let mut spawn_next = |set: &mut JoinSet<(usize, U)>| {
        if let Some((index, item)) = queued.next() {
            let task = Arc::clone(&task);
            set.spawn(async move { (index, task(item).await) });
        }
    };

    for _ in 0..concurrency {
        spawn_next(&mut set);
    }

    while let Some(joined) = set.join_next().await {
        let (index, value) = match joined {
            Ok(value) => value,
            // A panicking key function panics the caller, as it did before
            // the work moved onto a task.
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            Err(err) => unreachable!("the walker never aborts its own tasks: {err}"),
        };
        out[index] = Some(value);
        spawn_next(&mut set);
    }

    out.into_iter()
        .map(|value| value.expect("every spawned task reports its own index"))
        .collect()
}

/// One raw child of a directory, before it becomes a [`DirEntry`].
enum Raw {
    Entry(fs::DirEntry),
    Failed(Error),
}

/// Reads every child of `path` without resolving any of them.
///
/// Enumeration needs no `stat`, so it stays sequential; [`resolve`] is the
/// expensive half.
async fn read_raw(path: &Path, depth: usize) -> Result<Vec<Raw>> {
    let mut rd = fs::read_dir(path)
        .await
        .map_err(|err| Error::new_io_error(path.to_path_buf(), depth, err))?;

    let mut raw = Vec::new();
    loop {
        match rd.next_entry().await {
            Ok(Some(entry)) => raw.push(Raw::Entry(entry)),
            Ok(None) => break,
            Err(err) => raw.push(Raw::Failed(Error::new_io_error(
                path.to_path_buf(),
                depth,
                err,
            ))),
        }
    }
    Ok(raw)
}

/// Whether resolving one entry does I/O worth overlapping.
///
/// On Unix it does not unless a symlink must be followed: `d_type` and `d_ino`
/// answer everything `from_std` needs, and `is_hidden` is a name check, so a
/// task per entry would buy nothing and cost a spawn. On Windows every entry
/// needs its own metadata for `file_attributes`, so it always does.
#[cfg(unix)]
fn resolve_does_io(follow_links: bool, has_filter: bool) -> bool {
    follow_links || has_filter
}
#[cfg(windows)]
fn resolve_does_io(_follow_links: bool, _has_filter: bool) -> bool {
    true
}

/// Turns raw children into entries, at most `concurrency` at once, and drops
/// hidden entries when asked.
///
/// Both per-entry `stat`s live here — file type without `d_type`, and the
/// hidden flag on Windows — so a directory's overlap.
async fn resolve(raw: Vec<Raw>, opts: &ResolveOpts) -> Vec<Candidate> {
    // Sequential where there is no I/O to overlap; `map_bounded` treats 1 as
    // its fast path.
    let concurrency = if resolve_does_io(opts.follow_links, opts.filter.is_some()) {
        opts.concurrency
    } else {
        1
    };
    let ResolveOpts {
        depth,
        follow_links,
        skip_hidden,
        ..
    } = *opts;
    let filter = opts.filter.clone();

    let resolved = map_bounded(raw, concurrency, move |raw| {
        let filter = filter.clone();
        async move {
            let raw = match raw {
                // A child that never resolved cannot be filtered, so it is
                // always reported.
                Raw::Failed(err) => return Some(Candidate::kept(Err(err))),
                Raw::Entry(raw) => raw,
            };
            let entry = match DirEntry::<Async>::from_std(&raw, depth, follow_links).await {
                Ok(entry) => entry,
                Err(err) => return Some(Candidate::kept(Err(err))),
            };
            // Dropped here, not in the walker loop, so a hidden directory is
            // never descended into.
            if skip_hidden && entry.is_hidden().await {
                return None;
            }
            let Some(filter) = filter else {
                return Some(Candidate::kept(Ok(entry)));
            };
            match filter.decide(Arc::new(entry)).await {
                (Filtering::Continue, entry) => Some(Candidate::kept(Ok(entry))),
                (Filtering::IgnoreEntry, entry) if entry.is_dir() => {
                    Some(Candidate::descend_only(Ok(entry)))
                }
                (Filtering::IgnoreEntry | Filtering::IgnoreDir, _) => None,
            }
        }
    })
    .await;

    resolved.into_iter().flatten().collect()
}

/// Orders `entries` by `key`, awaiting each key once, at most
/// `order.concurrency` at a time.
///
/// The key sees the entry through an [`Arc`], recovered after every task is
/// joined, so metadata the key resolved stays cached and the recovery does not
/// copy.
async fn sort_by_key<K, Fut, F>(
    key: Arc<F>,
    entries: Vec<Candidate>,
    order: Order,
) -> Vec<Candidate>
where
    F: Fn(Arc<DirEntry<Async>>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = K> + Send + 'static,
    K: Ord + Send + 'static,
{
    let pending: Vec<Pending> = entries
        .into_iter()
        .map(|candidate| match candidate.entry {
            Ok(entry) => Pending::Keyed {
                entry: Arc::new(entry),
                yielded: candidate.yielded,
            },
            Err(err) => Pending::Failed(err),
        })
        .collect();

    let keyed: Vec<Arc<DirEntry<Async>>> = pending
        .iter()
        .filter_map(|pending| match pending {
            Pending::Keyed { entry, .. } => Some(Arc::clone(entry)),
            Pending::Failed(_) => None,
        })
        .collect();

    let keys = map_bounded(keyed, order.concurrency, move |entry| {
        let key = Arc::clone(&key);
        async move { key(entry).await }
    })
    .await;

    let mut keys = keys.into_iter();
    let mut ordered: Vec<Ordered<K>> = pending
        .into_iter()
        .map(|pending| match pending {
            // `None` sorts ahead of every `Some`.
            Pending::Failed(err) => Ordered {
                grouped: order.group_dir,
                key: None,
                name: error_name(&err),
                entry: Candidate::kept(Err(err)),
            },
            Pending::Keyed { entry, yielded } => {
                let key = keys.next().expect("one key per keyed entry");
                let entry = Arc::unwrap_or_clone(entry);
                Ordered {
                    grouped: order.grouped_entry(&entry),
                    key: Some(key),
                    name: entry.file_name().to_owned(),
                    entry: Candidate {
                        yielded,
                        entry: Ok(entry),
                    },
                }
            }
        })
        .collect();

    // Precedence: grouping, failed entries, key, file name.
    ordered.sort_by(|a, b| (a.grouped, &a.key, &a.name).cmp(&(b.grouped, &b.key, &b.name)));
    ordered.into_iter().map(|ordered| ordered.entry).collect()
}

/// Orders `entries` for [`group_dir`](AsyncWalkDir::group_dir) without a key:
/// the same order minus the key.
fn sort_grouped(entries: &mut [Candidate], order: Order) {
    entries.sort_by_cached_key(|candidate| {
        let entry = &candidate.entry;
        // `false` first, so a failed entry leads as it does with a key.
        (order.grouped(entry), entry.is_ok(), order_name(entry))
    });
}

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

/// Builder for configuring an asynchronous directory traversal.
#[derive(Debug)]
pub struct AsyncWalkDir {
    root: PathBuf,
    opts: WalkDirOptions,
    async_opts: AsyncOptions,
    sort_by: Option<Sorter>,
    filter: Option<Arc<Filter>>,
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
    /// use walkthrough::r#async::{AsyncWalkDir, Filtering};
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
    /// use walkthrough::r#async::AsyncWalkDir;
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

        // One `stat` per directory, skipped where a loop cannot occur; see
        // `LOOPS_NEED_FOLLOW_LINKS`.
        if self.opts.follow_links || !LOOPS_NEED_FOLLOW_LINKS {
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
