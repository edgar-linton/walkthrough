use std::{
    ffi::OsString,
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    vec,
};

use tokio::{fs, task::JoinSet};

use crate::{DirEntry, Error, Result, iter::WalkDirOptions};

/// Async state marker.
#[derive(Debug)]
pub struct Async;

/// Default for [`AsyncWalkDir::concurrency`].
///
/// Most of the win on a local filesystem is already taken at 32, where the
/// remaining cost is Tokio's blocking-pool dispatch rather than the syscall.
const DEFAULT_CONCURRENCY: usize = 32;

/// Whether a symlink loop is unreachable unless links are followed, and the
/// walker may therefore skip its ancestor bookkeeping — one `stat` per
/// descended directory — when [`AsyncWalkDir::follow_links`] is off.
///
/// True on Unix: a symlink is not descended into unless followed, and the
/// platform does not permit a hardlinked directory, so no cycle is reachable.
///
/// False on Windows, where a directory junction is a reparse point the platform
/// can report as a directory rather than as a link. A cycle through one is not
/// provably unreachable with `follow_links` off, and a missed loop is an
/// infinite walk — the wrong side to be wrong on to save one `stat`.
#[cfg(unix)]
const LOOPS_NEED_FOLLOW_LINKS: bool = true;
#[cfg(windows)]
const LOOPS_NEED_FOLLOW_LINKS: bool = false;

/// Configuration that only the asynchronous walker has.
///
/// Kept out of [`WalkDirOptions`] because the synchronous walker has nothing
/// to overlap and no pagination, so sharing the fields would mean three
/// options it must document as ignored.
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
    /// The grouping component of the order.
    ///
    /// `false` sorts first, so directories lead when
    /// [`group_dir`](AsyncWalkDir::group_dir) is set. Grouping is part of the
    /// comparison rather than a pass over the sorted result, which is what
    /// keeps the order total.
    fn grouped_entry(&self, entry: &DirEntry<Async>) -> bool {
        self.group_dir && !entry.is_dir()
    }

    /// As [`grouped_entry`](Self::grouped_entry), for an entry that may have
    /// failed outright — which is not a known directory.
    fn grouped(&self, entry: &Result<DirEntry<Async>>) -> bool {
        self.group_dir && !entry.as_ref().is_ok_and(DirEntry::is_dir)
    }
}

/// The name component of the order, and the reason the order is total: a file
/// name is unique within its directory.
///
/// A failed entry is ordered by the file name of the path it failed on, so it
/// still has a reproducible position.
fn order_name(entry: &Result<DirEntry<Async>>) -> OsString {
    match entry {
        Ok(entry) => entry.file_name().to_owned(),
        Err(err) => error_name(err),
    }
}

/// The name component of the order for an entry that failed outright: the file
/// name of the path it failed on.
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
    entry: Result<DirEntry<Async>>,
}

/// One child of a directory, before its key is known.
enum Pending {
    /// Failed outright, so no key is computed for it.
    Failed(Error),
    Keyed(Arc<DirEntry<Async>>),
}

type SortFuture = Pin<Box<dyn Future<Output = Vec<Result<DirEntry<Async>>>> + Send>>;

/// Orders the entries of one directory by an asynchronously computed key.
///
/// Holds the whole resolve-and-sort step rather than the key function, which is
/// why the key type does not appear here: [`AsyncWalkDir::sort_by`]
/// monomorphises the step and stores it as one closure, so the walker can order
/// entries by any [`Ord`] key without becoming generic over it.
///
/// `Sync` as well as `Send`, because the walker borrows the sorter across an
/// `await` and a walk has to stay spawnable.
#[allow(clippy::type_complexity)]
struct Sorter(Box<dyn Fn(Vec<Result<DirEntry<Async>>>, Order) -> SortFuture + Send + Sync>);

impl Sorter {
    async fn sort(
        &self,
        entries: Vec<Result<DirEntry<Async>>>,
        order: Order,
    ) -> Vec<Result<DirEntry<Async>>> {
        (self.0)(entries, order).await
    }
}

impl fmt::Debug for Sorter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sorter")
    }
}

/// Applies `task` to every item, at most `concurrency` of them in flight at
/// once, and returns the results in the order the items were given.
///
/// Results are placed by input position rather than by completion order, so
/// concurrency cannot influence the order the walker goes on to yield.
async fn map_bounded<T, U, Fut, F>(items: Vec<T>, concurrency: usize, task: F) -> Vec<U>
where
    T: Send + 'static,
    U: Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = U> + Send + 'static,
{
    // Not worth a task per item, and keeps `concurrency(1)` exactly as cheap
    // as the sequential walk it replaces.
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
            // A panicking key function used to panic the caller directly,
            // before any of this ran on a spawned task. Keep it that way
            // rather than swallowing the panic or reporting it as I/O.
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
/// Enumeration is cheap — one `getdents` batch per 32 entries, no `stat` — so
/// it stays sequential; the expensive part is [`resolve`].
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

/// Turns raw children into entries, resolving at most `concurrency` of them at
/// once, and drops hidden entries when asked.
///
/// Both per-entry resolves live here — the file type, which costs a `stat` on a
/// filesystem that does not populate `d_type`, and the hidden flag, which costs
/// one on Windows — so a directory's worth of them overlaps instead of adding
/// up.
async fn resolve(
    raw: Vec<Raw>,
    depth: usize,
    follow_links: bool,
    skip_hidden: bool,
    concurrency: usize,
) -> Vec<Result<DirEntry<Async>>> {
    let resolved = map_bounded(raw, concurrency, move |raw| async move {
        match raw {
            Raw::Failed(err) => Some(Err(err)),
            Raw::Entry(raw) => match DirEntry::<Async>::from_std(&raw, depth, follow_links).await {
                // A hidden directory is excluded from the walk entirely, so
                // dropping it here also prevents descent into it.
                Ok(entry) if skip_hidden && entry.is_hidden().await => None,
                resolved => Some(resolved),
            },
        }
    })
    .await;

    resolved.into_iter().flatten().collect()
}

/// Orders `entries` by `key`, awaiting the key of every entry exactly once and
/// at most `order.concurrency` at a time.
///
/// The entry is shared with the key function through an [`Arc`] and recovered
/// afterwards, so metadata the key resolved stays cached on the entry the walker
/// goes on to yield. Every task is joined before an entry is recovered, so the
/// key's clone is gone and the recovery does not copy.
async fn sort_by_key<K, Fut, F>(
    key: Arc<F>,
    entries: Vec<Result<DirEntry<Async>>>,
    order: Order,
) -> Vec<Result<DirEntry<Async>>>
where
    F: Fn(Arc<DirEntry<Async>>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = K> + Send + 'static,
    K: Ord + Send + 'static,
{
    let pending: Vec<Pending> = entries
        .into_iter()
        .map(|entry| match entry {
            Ok(entry) => Pending::Keyed(Arc::new(entry)),
            Err(err) => Pending::Failed(err),
        })
        .collect();

    let keyed: Vec<Arc<DirEntry<Async>>> = pending
        .iter()
        .filter_map(|pending| match pending {
            Pending::Keyed(entry) => Some(Arc::clone(entry)),
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
            // No key, and `None` sorts ahead of every `Some`, which is where a
            // failed entry belongs.
            Pending::Failed(err) => Ordered {
                grouped: order.group_dir,
                key: None,
                name: error_name(&err),
                entry: Err(err),
            },
            Pending::Keyed(shared) => {
                let key = keys.next().expect("one key per keyed entry");
                let entry = Arc::unwrap_or_clone(shared);
                Ordered {
                    grouped: order.grouped_entry(&entry),
                    key: Some(key),
                    name: entry.file_name().to_owned(),
                    entry: Ok(entry),
                }
            }
        })
        .collect();

    // Total, in precedence order: grouping, then failed entries, then the key,
    // then the file name — which no two entries of one directory share.
    ordered.sort_by(|a, b| (a.grouped, &a.key, &a.name).cmp(&(b.grouped, &b.key, &b.name)));
    ordered.into_iter().map(|ordered| ordered.entry).collect()
}

/// Orders `entries` when [`group_dir`](AsyncWalkDir::group_dir) is set but no
/// sort key is, by the same total order minus the key.
fn sort_grouped(entries: &mut [Result<DirEntry<Async>>], order: Order) {
    entries.sort_by_cached_key(|entry| {
        // `false` first, so a failed entry leads as it does with a key, where
        // its `None` sorts ahead of every `Some`.
        (order.grouped(entry), entry.is_ok(), order_name(entry))
    });
}

// Stack item: pre-ordered entries (when sort_by or group_dir is set) or a live
// ReadDir handle (the common unordered case, which avoids collecting upfront).
enum DirStream {
    Sorted(vec::IntoIter<Result<DirEntry<Async>>>),
    Live {
        rd: Box<fs::ReadDir>,
        path: PathBuf,
        depth: usize,
        follow_links: bool,
        skip_hidden: bool,
        concurrency: usize,
        // Resolved and not yet yielded. At most `concurrency` entries, so the
        // streaming path still never materialises a directory.
        ready: vec::IntoIter<Result<DirEntry<Async>>>,
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
                skip_hidden,
                concurrency,
                ready,
            } => loop {
                if let Some(entry) = ready.next() {
                    return Some(entry);
                }

                // Refill: read up to `concurrency` raw entries, then resolve
                // them as one batch so their I/O overlaps.
                let mut raw = Vec::with_capacity(*concurrency);
                while raw.len() < *concurrency {
                    match rd.next_entry().await {
                        Ok(Some(entry)) => raw.push(Raw::Entry(entry)),
                        Ok(None) => break,
                        Err(err) => {
                            raw.push(Raw::Failed(Error::new_io_error(path.clone(), *depth, err)));
                            break;
                        }
                    }
                }

                if raw.is_empty() {
                    return None;
                }

                *ready = resolve(raw, *depth, *follow_links, *skip_hidden, *concurrency)
                    .await
                    .into_iter();
                // A batch can be emptied entirely by `skip_hidden`, so loop
                // rather than returning `None` on an empty one.
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
}

impl AsyncWalkDir {
    /// Creates a new traversal rooted at `root`.
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            opts: WalkDirOptions::default(),
            async_opts: AsyncOptions::default(),
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

    /// Sets how many entries of one directory may have I/O in flight at once.
    ///
    /// Defaults to 32, and is clamped to at least 1. The bounded work is every
    /// per-entry resolve the walker performs: the [`sort_by`](Self::sort_by)
    /// key, the file type on a filesystem that does not populate `d_type`, and
    /// the hidden flag on Windows. Without it those are awaited one entry at a
    /// time, so a directory's latency is the sum of its entries' rather than
    /// the longest of them.
    ///
    /// This matters in proportion to how slow one entry is. A local filesystem
    /// resolves metadata in microseconds and sees roughly a threefold
    /// improvement, bounded by Tokio's blocking-pool dispatch rather than by
    /// the syscall. A network mount or FUSE layer where one `stat` costs
    /// milliseconds is bound by round trips instead, and 128–512 is reasonable
    /// there: 2000 entries at 5 ms each take 10 s sequentially and 175 ms at a
    /// concurrency of 64.
    ///
    /// There is deliberately no unbounded setting, since a directory with a
    /// million children would then start a million concurrent resolves.
    ///
    /// Concurrency never affects the order entries are yielded in; results are
    /// placed by directory position, not by completion.
    ///
    /// The synchronous walker has no counterpart, having nothing to overlap.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn run() -> walkthrough::Result<()> {
    /// use walkthrough::AsyncWalkDir;
    ///
    /// // A listing on a network mount: bound by round trips, not by CPU.
    /// let mut walker = AsyncWalkDir::new("/mnt/share")
    ///     .max_depth(1)
    ///     .concurrency(256)
    ///     .sort_by(|entry| async move { entry.metadata().await.map(|m| m.len()).unwrap_or(0) })
    ///     .walker()
    ///     .await;
    /// # while let Some(entry) = walker.next().await { entry?; }
    /// # Ok(())
    /// # }
    /// ```
    pub fn concurrency(mut self, n: usize) -> Self {
        self.async_opts.concurrency = n.max(1);
        self
    }

    /// Yields at most `limit` entries, then ends the walk.
    ///
    /// Counted over the entries the walk yields, across the whole traversal
    /// rather than per directory, so it composes with
    /// [`min_depth`](Self::min_depth) and [`skip_hidden`](Self::skip_hidden)
    /// the way SQL's `LIMIT` composes with a `WHERE` clause. Errors the walk
    /// reports are entries for this purpose. `limit(0)` yields nothing.
    ///
    /// A limited walk stops descending once it is satisfied, so it is cheaper
    /// than an unlimited one — but it still resolves every entry of every
    /// directory it opens, because ordering by a key requires the key of every
    /// candidate. `limit` bounds what a caller receives, not what the
    /// filesystem is asked; [`concurrency`](Self::concurrency) is what makes
    /// the latter fast.
    pub fn limit(mut self, limit: usize) -> Self {
        self.async_opts.limit = limit;
        self
    }

    /// Skips the first `offset` entries the walk would yield.
    ///
    /// Together with [`limit`](Self::limit) this is `skip(offset).take(limit)`
    /// over the unpaginated walk, which is what makes it interchangeable with
    /// the `OFFSET`/`LIMIT` pair of an SQL-backed listing. An `offset` past the
    /// end of the walk yields nothing, and is not an error.
    ///
    /// Two caveats it shares with SQL offsets, both worth passing on to
    /// whoever consumes the pages:
    ///
    /// - Entries created or removed between two requests can make a page skip
    ///   or repeat an entry. An offset is a position, not a snapshot.
    /// - It is only meaningful over a reproducible order. With neither
    ///   [`sort_by`](Self::sort_by) nor [`group_dir`](Self::group_dir),
    ///   entries arrive in filesystem order, which is not reproducible across
    ///   two `read_dir` calls of the same directory. Set an ordering when
    ///   paginating.
    pub fn offset(mut self, offset: usize) -> Self {
        self.async_opts.offset = offset;
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
    /// comparison. Keys are resolved [`concurrency`](Self::concurrency) at a
    /// time, so a directory costs roughly one key's latency per batch instead
    /// of one per entry. Keys that await nothing cost nothing beyond the key
    /// itself.
    ///
    /// The entry arrives as an [`Arc`] so the key may hold it across an `await`;
    /// metadata resolved inside the key stays cached on the entry the walk yields.
    /// Compound orderings are tuple keys, and descending order is
    /// [`Reverse`](std::cmp::Reverse). Calling this more than once keeps the last
    /// key.
    ///
    /// The resulting order is total, and is, in precedence order:
    /// [`group_dir`](Self::group_dir) if set, then entries that failed outright
    /// — for which no key is computed — then the key, then the file name. The
    /// name is what makes it total, and therefore reproducible between two
    /// walks of the same directory, which is what
    /// [`offset`](Self::offset) needs.
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
        self.sort_by = Some(Sorter(Box::new(move |entries, order| {
            Box::pin(sort_by_key(Arc::clone(&key), entries, order))
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
            async_opts: self.async_opts,
            sort_by: self.sort_by,
            yielded: 0,
            skipped: 0,
        }
    }
}

/// Async stateful walker produced by [`AsyncWalkDir::walker`].
#[derive(Debug)]
pub struct AsyncWalker {
    opts: WalkDirOptions,
    async_opts: AsyncOptions,
    sort_by: Option<Sorter>,
    ancestors: Vec<crate::Ancestor>,
    stack: Vec<DirStream>,
    start: Option<Result<DirEntry<Async>>>,
    yielded: usize,
    skipped: usize,
}

impl AsyncWalker {
    fn order(&self) -> Order {
        Order {
            group_dir: self.opts.group_dir,
            concurrency: self.async_opts.concurrency,
        }
    }

    async fn push_dir(&mut self, entry: &DirEntry<Async>) -> Result<()> {
        let depth = entry.depth();

        // Resolving an ancestor costs one `stat` per directory, so it is worth
        // skipping where it cannot fire — which is platform-dependent; see
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

        if self.sort_by.is_some() || self.opts.group_dir {
            let entries = self.collect_ordered(&path, child_depth).await?;
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
                skip_hidden: self.opts.skip_hidden,
                concurrency: self.async_opts.concurrency,
                ready: Vec::new().into_iter(),
            });
        }
        Ok(())
    }

    async fn collect_ordered(
        &self,
        path: &Path,
        depth: usize,
    ) -> Result<Vec<Result<DirEntry<Async>>>> {
        let order = self.order();
        let raw = read_raw(path, depth).await?;
        let mut entries = resolve(
            raw,
            depth,
            self.opts.follow_links,
            self.opts.skip_hidden,
            order.concurrency,
        )
        .await;

        match self.sort_by {
            Some(ref sorter) => entries = sorter.sort(entries, order).await,
            None => sort_grouped(&mut entries, order),
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
    ///
    /// With [`limit`](AsyncWalkDir::limit) or [`offset`](AsyncWalkDir::offset)
    /// set, this is `skip(offset).take(limit)` over the entries the walk would
    /// otherwise produce. Skipped entries are still descended into, so a page
    /// is a window over the traversal rather than a truncation of it.
    pub async fn next(&mut self) -> Option<Result<DirEntry<Async>>> {
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

            // `skip_hidden` is applied while a directory's entries are
            // resolved, so nothing hidden reaches here.
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
    }
}
