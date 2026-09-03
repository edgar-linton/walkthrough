use std::{ffi::OsString, future::Future, path::Path, sync::Arc};

use tokio::{fs, task::JoinSet};

use super::{
    iter::{Filter, Filtering},
    state::Async,
};
use crate::{DirEntry, Error, Result};

/// Everything ordering one directory needs beyond the entries themselves.
#[derive(Debug, Clone, Copy)]
pub(super) struct Order {
    pub(super) group_dir: bool,
    pub(super) concurrency: usize,
}

impl Order {
    /// The grouping component of the order; `false` sorts first, so
    /// directories lead when [`group_dir`](super::AsyncWalkDir::group_dir) is set.
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

/// A resolved child together with whether the walk yields it.
pub(super) struct Candidate {
    /// `false` for [`Filtering::IgnoreEntry`] on a directory: descended into,
    /// as an entry above `min_depth` is, but never yielded.
    pub(super) yielded: bool,
    pub(super) entry: Result<DirEntry<Async>>,
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
pub(super) struct ResolveOpts {
    pub(super) depth: usize,
    pub(super) follow_links: bool,
    pub(super) skip_hidden: bool,
    pub(super) concurrency: usize,
    pub(super) filter: Option<Arc<Filter>>,
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
pub(super) enum Raw {
    Entry(fs::DirEntry),
    Failed(Error),
}

/// Reads every child of `path` without resolving any of them.
///
/// Enumeration needs no `stat`, so it stays sequential; [`resolve`] is the
/// expensive half.
pub(super) async fn read_raw(path: &Path, depth: usize) -> Result<Vec<Raw>> {
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
pub(super) async fn resolve(raw: Vec<Raw>, opts: &ResolveOpts) -> Vec<Candidate> {
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
pub(super) async fn sort_by_key<K, Fut, F>(
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

/// Orders `entries` for [`group_dir`](super::AsyncWalkDir::group_dir) without
/// a key: the same order minus the key.
pub(super) fn sort_grouped(entries: &mut [Candidate], order: Order) {
    entries.sort_by_cached_key(|candidate| {
        let entry = &candidate.entry;
        // `false` first, so a failed entry leads as it does with a key.
        (order.grouped(entry), entry.is_ok(), order_name(entry))
    });
}
