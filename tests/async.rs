#[cfg(unix)]
use std::os::unix::fs as unix_fs;
#[cfg(windows)]
use std::os::windows::fs as win_fs;
use std::{collections::BTreeSet, fs, path::Path, sync::Arc};

use tempfile::TempDir;
use walkthrough::{Async, AsyncWalkDir, AsyncWalker, DirEntry, ErrorKind, Result};

// The async types are public at the crate root and in `aio`. Both paths are
// API, and the root re-export was dropped once without a test noticing, so
// pin them to one type here; the rest of the suite covers the root path only.
fn _both_paths_name_one_type(builder: walkthrough::aio::AsyncWalkDir) -> AsyncWalkDir {
    builder
}

// ---------------------------------------------------------------------------
// Fixture helpers — identical structure to the sync suite
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn set_hidden(path: &Path) {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN);
    }
}

#[cfg(unix)]
fn set_hidden(_path: &Path) {}

/// ```text
/// root/
///   .hidden0a          (hidden on all platforms)
///   dir1a/
///     dir2a/
///       file2a_a
///     file1a_a
///   dir1b/
///     file1b_a
///   file0a
/// ```
fn basic_tree() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    fs::write(r.join("file0a"), "").unwrap();
    let hidden = r.join(".hidden0a");
    fs::write(&hidden, "").unwrap();
    set_hidden(&hidden);
    let dir1a = r.join("dir1a");
    fs::create_dir_all(dir1a.join("dir2a")).unwrap();
    fs::write(dir1a.join("file1a_a"), "").unwrap();
    fs::write(dir1a.join("dir2a").join("file2a_a"), "").unwrap();
    let dir1b = r.join("dir1b");
    fs::create_dir(&dir1b).unwrap();
    fs::write(dir1b.join("file1b_a"), "").unwrap();
    tmp
}

/// ```text
/// root/
///   link_to_file  ->  real_file
///   link_to_dir   ->  real_dir/
///   real_dir/
///     inside
///   real_file
/// ```
fn symlink_tree() -> Option<TempDir> {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    fs::write(r.join("real_file"), "hello").unwrap();
    #[cfg(unix)]
    unix_fs::symlink(r.join("real_file"), r.join("link_to_file")).unwrap();
    #[cfg(windows)]
    win_fs::symlink_file(r.join("real_file"), r.join("link_to_file")).ok()?;
    let real_dir = r.join("real_dir");
    fs::create_dir(&real_dir).unwrap();
    fs::write(real_dir.join("inside"), "").unwrap();
    #[cfg(unix)]
    unix_fs::symlink(&real_dir, r.join("link_to_dir")).unwrap();
    #[cfg(windows)]
    win_fs::symlink_dir(&real_dir, r.join("link_to_dir")).ok()?;
    Some(tmp)
}

/// ```text
/// root/
///   a/
///     b/
///       loop  ->  root/a
/// ```
fn loop_tree() -> Option<TempDir> {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    let a = r.join("a");
    fs::create_dir_all(a.join("b")).unwrap();
    #[cfg(unix)]
    unix_fs::symlink(&a, a.join("b").join("loop")).unwrap();
    #[cfg(windows)]
    win_fs::symlink_dir(&a, a.join("b").join("loop")).ok()?;
    Some(tmp)
}

/// ```text
/// root/
///   a/
///     b/
///       j  ->  root/a   (directory junction)
/// ```
/// A junction is a reparse point that needs no privileges to create, unlike a
/// symlink, so this fixture is available wherever [`loop_tree`] is not.
/// Returns `None` if `mklink` is unavailable.
#[cfg(windows)]
fn junction_loop_tree() -> Option<TempDir> {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    let a = r.join("a");
    fs::create_dir_all(a.join("b")).unwrap();
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(a.join("b").join("j"))
        .arg(&a)
        .output()
        .ok()?;
    out.status.success().then_some(tmp)
}

/// ```text
/// root/
///   dangling  ->  __no_such_target__
/// ```
fn dangling_symlink_tree() -> Option<TempDir> {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    // A Windows symlink picks file-vs-dir at creation; a file link is the
    // case that used to escape resolution, so use it here.
    #[cfg(unix)]
    unix_fs::symlink("__no_such_target__", r.join("dangling")).unwrap();
    #[cfg(windows)]
    win_fs::symlink_file("__no_such_target__", r.join("dangling")).ok()?;
    Some(tmp)
}

// ---------------------------------------------------------------------------
// Async walk helpers
// ---------------------------------------------------------------------------

async fn walk_all(walk: AsyncWalkDir) -> Vec<Result<DirEntry<Async>>> {
    let mut walker: AsyncWalker = walk.walker();
    let mut out = Vec::new();
    while let Some(res) = walker.next_entry().await {
        out.push(res);
    }
    out
}

async fn walk_ok(walk: AsyncWalkDir) -> Vec<DirEntry<Async>> {
    walk_all(walk)
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect()
}

async fn rel_paths(root: &Path, walk: AsyncWalkDir) -> BTreeSet<String> {
    walk_ok(walk)
        .await
        .into_iter()
        .filter(|e| e.depth() > 0)
        .map(|e| {
            e.path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Basic traversal — exercises DirStream::Live (no sort configured)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_all_entries_are_visited() {
    let tmp = basic_tree();
    let paths = rel_paths(tmp.path(), AsyncWalkDir::new(tmp.path())).await;
    assert_eq!(
        paths,
        BTreeSet::from([
            ".hidden0a".into(),
            "dir1a".into(),
            "dir1a/dir2a".into(),
            "dir1a/dir2a/file2a_a".into(),
            "dir1a/file1a_a".into(),
            "dir1b".into(),
            "dir1b/file1b_a".into(),
            "file0a".into(),
        ])
    );
}

#[tokio::test]
async fn test_async_root_is_yielded_at_depth_0() {
    let tmp = basic_tree();
    let first = walk_ok(AsyncWalkDir::new(tmp.path()))
        .await
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(first.depth(), 0);
    assert_eq!(first.path(), tmp.path());
}

// ---------------------------------------------------------------------------
// Depth limits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_max_depth_1_does_not_descend_into_subdirs() {
    let tmp = basic_tree();
    let paths = rel_paths(tmp.path(), AsyncWalkDir::new(tmp.path()).max_depth(1)).await;
    assert_eq!(
        paths,
        BTreeSet::from([
            ".hidden0a".into(),
            "dir1a".into(),
            "dir1b".into(),
            "file0a".into(),
        ])
    );
}

#[tokio::test]
async fn test_async_max_depth_0_yields_only_root() {
    let tmp = basic_tree();
    let entries = walk_ok(AsyncWalkDir::new(tmp.path()).max_depth(0)).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].depth(), 0);
}

#[tokio::test]
async fn test_async_min_depth_1_skips_root() {
    let tmp = basic_tree();
    let entries = walk_ok(AsyncWalkDir::new(tmp.path()).min_depth(1)).await;
    assert!(entries.iter().all(|e| e.depth() >= 1));
    assert_eq!(entries.len(), 8);
}

#[tokio::test]
async fn test_async_min_depth_2_skips_depth_0_and_1_entries() {
    let tmp = basic_tree();
    let entries = walk_ok(AsyncWalkDir::new(tmp.path()).min_depth(2)).await;
    assert!(entries.iter().all(|e| e.depth() >= 2));
    // dir2a, file1a_a, file1b_a, file2a_a
    assert_eq!(entries.len(), 4);
}

// ---------------------------------------------------------------------------
// Filtering — exercises is_hidden() and the skip_hidden branch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_skip_hidden_excludes_dot_entries() {
    let tmp = basic_tree();
    let paths = rel_paths(tmp.path(), AsyncWalkDir::new(tmp.path()).skip_hidden(true)).await;
    assert!(!paths.contains(".hidden0a"));
    assert!(paths.contains("file0a"));
    assert!(paths.contains("dir1a"));
}

#[tokio::test]
async fn test_async_skip_hidden_root_is_exempt() {
    let tmp = TempDir::new().unwrap();
    let hidden_root = tmp.path().join(".hidden_root");
    fs::create_dir(&hidden_root).unwrap();
    set_hidden(&hidden_root);
    fs::write(hidden_root.join("file.txt"), "").unwrap();

    let entries = walk_ok(AsyncWalkDir::new(&hidden_root).skip_hidden(true)).await;

    // The root itself is hidden but still yielded and descended into, since
    // walking it was explicit — skip_hidden only filters what's discovered
    // during recursion.
    assert!(
        entries
            .iter()
            .any(|e| e.depth() == 0 && e.path() == hidden_root)
    );
    assert!(
        entries
            .iter()
            .any(|e| e.depth() == 1 && e.file_name().to_string_lossy() == "file.txt")
    );
}

// ---------------------------------------------------------------------------
// Sorting — exercises DirStream::Sorted via collect_sorted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_sort_by_name_orders_siblings_alphabetically() {
    let tmp = basic_tree();
    // sort_by forces DirStream::Sorted for every directory level.
    let depth1: Vec<String> =
        walk_ok(AsyncWalkDir::new(tmp.path()).sort_by(|e| async move { e.file_name().to_owned() }))
            .await
            .into_iter()
            .filter(|e| e.depth() == 1)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

    let mut sorted = depth1.clone();
    sorted.sort();
    assert_eq!(depth1, sorted);
}

#[tokio::test]
async fn test_async_group_dir_places_dirs_before_files() {
    let tmp = basic_tree();
    // group_dir also forces DirStream::Sorted.
    let depth1 = walk_ok(AsyncWalkDir::new(tmp.path()).group_dir(true))
        .await
        .into_iter()
        .filter(|e| e.depth() == 1)
        .collect::<Vec<_>>();

    let first_file = depth1.iter().position(|e| !e.is_dir());
    let last_dir = depth1.iter().rposition(|e| e.is_dir());
    if let (Some(f), Some(d)) = (first_file, last_dir) {
        assert!(d < f, "a directory appeared after a file at depth 1");
    }
}

/// A tree whose file sizes are deliberately at odds with their names, so an
/// ordering by size cannot pass by accident.
///
/// ```text
/// root/
///   big     (3 bytes)
///   small   (1 byte)
///   sub/
///     medium  (2 bytes)
///     tiny    (0 bytes)
/// ```
fn sized_tree() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    fs::write(r.join("big"), "aaa").unwrap();
    fs::write(r.join("small"), "a").unwrap();
    let sub = r.join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("medium"), "aa").unwrap();
    fs::write(sub.join("tiny"), "").unwrap();
    tmp
}

/// A key that awaits — an `async fn` item rather than a closure, since either is
/// accepted. Unreadable metadata sorts as `0` rather than failing the walk.
async fn size_key(entry: Arc<DirEntry<Async>>) -> u64 {
    entry.metadata().await.map(|m| m.len()).unwrap_or(0)
}

async fn file_names_at_depth1(walk: AsyncWalkDir) -> Vec<String> {
    walk_ok(walk)
        .await
        .into_iter()
        .filter(|e| e.depth() == 1 && !e.is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// The regression these two exist for: an ordering by size is expressible at all.
/// A synchronous comparator cannot await `metadata`, and blocking on it from
/// inside one panics with "Cannot start a runtime from within a runtime" — on
/// either runtime flavour, which surfaces as a `500` when the walk runs inside a
/// request handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_async_sort_by_size_on_a_multi_thread_runtime() {
    let tmp = sized_tree();

    let names = file_names_at_depth1(AsyncWalkDir::new(tmp.path()).sort_by(size_key)).await;

    assert_eq!(names, vec!["small", "big"]);
}

/// The same key on a current-thread runtime — the flavour actix-web and friends
/// drive request handlers on.
#[tokio::test(flavor = "current_thread")]
async fn test_async_sort_by_size_on_a_current_thread_runtime() {
    let tmp = sized_tree();

    let names = file_names_at_depth1(AsyncWalkDir::new(tmp.path()).sort_by(size_key)).await;

    assert_eq!(names, vec!["small", "big"]);
}

/// A configured key must not cost the walk its `Send`ness: `tokio::spawn` requires
/// it, which is what the `Send` bounds on the key and its future are there for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_async_walk_with_sort_by_is_spawnable() {
    let tmp = sized_tree();
    let root = tmp.path().to_path_buf();

    let names = tokio::spawn(async move {
        file_names_at_depth1(AsyncWalkDir::new(root).sort_by(size_key)).await
    })
    .await
    .unwrap();

    assert_eq!(names, vec!["small", "big"]);
}

/// The shape a request handler has: a current-thread runtime driving a `!Send`
/// task. This is where blocking on `metadata` inside a comparator used to panic.
#[tokio::test(flavor = "current_thread")]
async fn test_async_walk_with_sort_by_runs_in_a_local_task() {
    let tmp = sized_tree();
    let root = tmp.path().to_path_buf();

    let names = tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                file_names_at_depth1(AsyncWalkDir::new(root).sort_by(size_key)).await
            })
            .await
            .unwrap()
        })
        .await;

    assert_eq!(names, vec!["small", "big"]);
}

/// The ordering is per directory — not one global ordering across the walk.
/// `sub/` is followed by its own entries, smallest first.
#[tokio::test]
async fn test_async_sort_by_orders_within_each_directory() {
    let tmp = sized_tree();

    let names: Vec<String> = walk_ok(AsyncWalkDir::new(tmp.path()).sort_by(size_key))
        .await
        .into_iter()
        .filter(|e| e.depth() > 0)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    let sub = names.iter().position(|n| n == "sub").unwrap();
    let tiny = names.iter().position(|n| n == "tiny").unwrap();
    let medium = names.iter().position(|n| n == "medium").unwrap();
    assert!(
        sub < tiny && tiny < medium,
        "`sub/` must be followed by its own entries, smallest first: {names:?}"
    );
}

/// Descending order is the key's business, not a second method.
#[tokio::test]
async fn test_async_sort_by_reverses_with_reverse() {
    use std::cmp::Reverse;

    let tmp = sized_tree();

    let names = file_names_at_depth1(
        AsyncWalkDir::new(tmp.path()).sort_by(|e| async move { Reverse(size_key(e).await) }),
    )
    .await;

    assert_eq!(names, vec!["big", "small"]);
}

/// A tuple key mixes an awaited property with a synchronous one — the ordering a
/// metadata-only key could not express. Both files are 1 byte, so the name breaks
/// the tie; `zzz` is 3 bytes and sorts last despite its name.
#[tokio::test]
async fn test_async_sort_by_compound_key_breaks_size_ties_by_name() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("zzz"), "aaa").unwrap();
    fs::write(tmp.path().join("beta"), "a").unwrap();
    fs::write(tmp.path().join("alpha"), "a").unwrap();

    let names =
        file_names_at_depth1(AsyncWalkDir::new(tmp.path()).sort_by(|e| async move {
            (size_key(Arc::clone(&e)).await, e.file_name().to_owned())
        }))
        .await;

    assert_eq!(names, vec!["alpha", "beta", "zzz"]);
}

/// `group_dir` is applied after the key and still wins over it.
#[tokio::test]
async fn test_async_sort_by_composes_with_group_dir() {
    let tmp = sized_tree();

    let depth1 = walk_ok(
        AsyncWalkDir::new(tmp.path())
            .sort_by(size_key)
            .group_dir(true),
    )
    .await
    .into_iter()
    .filter(|e| e.depth() == 1)
    .collect::<Vec<_>>();

    assert!(depth1[0].is_dir(), "the directory must come first");
    let files: Vec<String> = depth1
        .iter()
        .filter(|e| !e.is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, vec!["small", "big"], "the key orders the files");
}

/// The `Arc` the key receives is the entry the walk goes on to yield, so metadata
/// the key resolved is still cached on it afterwards. Observable by deleting the
/// file first: an uncached read would have to `stat` it again and fail.
#[tokio::test]
async fn test_async_sort_by_leaves_metadata_cached_on_the_entry() {
    let tmp = sized_tree();
    let entry = walk_ok(AsyncWalkDir::new(tmp.path()).min_depth(1).sort_by(size_key))
        .await
        .into_iter()
        .find(|e| !e.is_dir())
        .unwrap();

    fs::remove_file(entry.path()).unwrap();

    assert!(
        entry.metadata().await.is_ok(),
        "the key resolved this entry's metadata, so it must still be cached"
    );
}

/// A second `sort_by` replaces the first, including when the two keys are of
/// different types — the key type is erased, so only the last one survives.
#[tokio::test]
async fn test_async_last_sort_by_call_wins() {
    let tmp = sized_tree();

    let by_name = file_names_at_depth1(
        AsyncWalkDir::new(tmp.path())
            .sort_by(size_key)
            .sort_by(|e| async move { e.file_name().to_owned() }),
    )
    .await;
    let by_size = file_names_at_depth1(
        AsyncWalkDir::new(tmp.path())
            .sort_by(|e| async move { e.file_name().to_owned() })
            .sort_by(size_key),
    )
    .await;

    assert_eq!(by_name, vec!["big", "small"], "name key configured last");
    assert_eq!(by_size, vec!["small", "big"], "size key configured last");
}

// ---------------------------------------------------------------------------
// Sorted + unsorted in the same walk: root level unsorted, subdir sorted
// (covers both DirStream variants being used in one traversal)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_full_set_reached_with_sort_and_group() {
    let tmp = basic_tree();
    // Using both sort_by and group_dir ensures every directory opened goes
    // through collect_sorted, while still visiting the full tree.
    let paths = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path())
            .sort_by(|e| async move { e.file_name().to_owned() })
            .group_dir(true),
    )
    .await;
    assert_eq!(paths.len(), 8);
}

// ---------------------------------------------------------------------------
// Symlink handling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_follow_links_false_does_not_descend_into_symlinked_dir() {
    let Some(tmp) = symlink_tree() else { return };
    let paths = rel_paths(tmp.path(), AsyncWalkDir::new(tmp.path())).await;
    assert!(paths.contains("link_to_dir"));
    assert_eq!(paths.iter().filter(|p| p.ends_with("inside")).count(), 1);
}

#[tokio::test]
async fn test_async_follow_links_true_descends_into_symlinked_dir() {
    let Some(tmp) = symlink_tree() else { return };
    let paths = rel_paths(tmp.path(), AsyncWalkDir::new(tmp.path()).follow_links(true)).await;
    assert!(paths.contains("link_to_dir"));
    assert_eq!(paths.iter().filter(|p| p.ends_with("inside")).count(), 2);
}

#[tokio::test]
async fn test_async_follow_links_true_resolves_symlink_to_file() {
    let Some(tmp) = symlink_tree() else { return };

    // Following a link makes the entry look like its target on every
    // platform, not just Unix.
    let followed = walk_ok(AsyncWalkDir::new(tmp.path()).follow_links(true))
        .await
        .into_iter()
        .find(|e| e.file_name() == "link_to_file")
        .unwrap();
    assert!(followed.file_type().is_file());
    assert!(!followed.file_type().is_symlink());

    let unfollowed = walk_ok(AsyncWalkDir::new(tmp.path()))
        .await
        .into_iter()
        .find(|e| e.file_name() == "link_to_file")
        .unwrap();
    assert!(unfollowed.file_type().is_symlink());
}

#[tokio::test]
async fn test_async_follow_links_loop_returns_error() {
    let Some(tmp) = loop_tree() else { return };
    let results = walk_all(AsyncWalkDir::new(tmp.path()).follow_links(true)).await;
    assert!(results.into_iter().any(|r| r.is_err_and(|e| e.is_loop())));
}

#[tokio::test]
async fn test_async_follow_links_dangling_symlink_yields_io_error() {
    let Some(tmp) = dangling_symlink_tree() else {
        return;
    };
    let errors: Vec<_> = walk_all(AsyncWalkDir::new(tmp.path()).follow_links(true))
        .await
        .into_iter()
        .filter_map(|r| r.err())
        .collect();
    assert!(!errors.is_empty());
    assert!(errors.iter().all(|e| e.is_io()));
}

// ---------------------------------------------------------------------------
// Error API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_io_error_exposes_all_accessors() {
    let results = walk_all(AsyncWalkDir::new("__nonexistent_walkthrough_xyz__")).await;
    assert_eq!(results.len(), 1);
    let err = results.into_iter().next().unwrap().unwrap_err();

    assert!(err.is_io());
    assert!(!err.is_loop());
    assert_eq!(err.depth(), 0);
    assert!(err.path().ends_with("__nonexistent_walkthrough_xyz__"));
    assert!(matches!(err.kind(), ErrorKind::Io(_)));
    assert!(err.io_error().is_some());
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn test_async_io_error_into_io_error_is_some() {
    let err = walk_all(AsyncWalkDir::new("__nonexistent_walkthrough_abc__"))
        .await
        .into_iter()
        .next()
        .unwrap()
        .unwrap_err();
    assert!(err.into_io_error().is_some());
}

#[tokio::test]
async fn test_async_loop_error_exposes_all_accessors() {
    let Some(tmp) = loop_tree() else { return };
    let err = walk_all(AsyncWalkDir::new(tmp.path()).follow_links(true))
        .await
        .into_iter()
        .find_map(|r| r.err().filter(|e| e.is_loop()))
        .unwrap();

    assert!(err.is_loop());
    assert!(!err.is_io());
    assert_eq!(err.depth(), 3);
    assert!(matches!(err.kind(), ErrorKind::LoopDetected));
    assert!(err.io_error().is_none());
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn test_async_loop_error_into_io_error_is_none() {
    let Some(tmp) = loop_tree() else { return };
    let err = walk_all(AsyncWalkDir::new(tmp.path()).follow_links(true))
        .await
        .into_iter()
        .find_map(|r| r.err().filter(|e| e.is_loop()))
        .unwrap();
    assert!(err.into_io_error().is_none());
}

// ---------------------------------------------------------------------------
// DirEntry API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_direntry_into_path_matches_path() {
    let tmp = basic_tree();
    let entry = walk_ok(AsyncWalkDir::new(tmp.path()))
        .await
        .into_iter()
        .next()
        .unwrap();
    let expected = entry.path().to_path_buf();
    assert_eq!(entry.into_path(), expected);
}

#[tokio::test]
async fn test_async_direntry_file_type_distinguishes_dirs_and_files() {
    let tmp = basic_tree();
    let entries = walk_ok(AsyncWalkDir::new(tmp.path())).await;

    let root = entries.iter().find(|e| e.depth() == 0).unwrap();
    assert!(root.file_type().is_dir());

    let file = entries
        .iter()
        .find(|e| e.path().ends_with("file0a"))
        .unwrap();
    assert!(file.file_type().is_file());
}

#[tokio::test]
async fn test_async_direntry_metadata_is_ok_for_all_entries() {
    let tmp = basic_tree();
    // follow_links(true) exercises the non-cached metadata branch on every entry.
    for entry in walk_ok(AsyncWalkDir::new(tmp.path()).follow_links(true)).await {
        assert!(
            entry.metadata().await.is_ok(),
            "metadata failed for {:?}",
            entry.path()
        );
    }
}

#[tokio::test]
async fn test_async_metadata_second_call_is_ok() {
    let tmp = basic_tree();
    // Call metadata() twice on the root entry.  The second call hits the cached
    // path (OnceCell on unix, clone on windows); both must succeed.
    let entry = walk_ok(AsyncWalkDir::new(tmp.path()))
        .await
        .into_iter()
        .next()
        .unwrap();
    assert!(entry.metadata().await.is_ok());
    assert!(entry.metadata().await.is_ok());
}

// ---------------------------------------------------------------------------
// Walker edge cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_nonexistent_root_yields_single_io_error() {
    let results = walk_all(AsyncWalkDir::new("__nonexistent_walkthrough_root__")).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].as_ref().unwrap_err().is_io());
}

#[tokio::test]
async fn test_async_file_as_root_yields_single_non_dir_entry() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("lone_file");
    fs::write(&file, "").unwrap();

    let entries = walk_ok(AsyncWalkDir::new(&file)).await;
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].is_dir());
    assert_eq!(entries[0].depth(), 0);
}

#[tokio::test]
async fn test_async_empty_directory_yields_only_root() {
    let tmp = TempDir::new().unwrap();
    let entries = walk_ok(AsyncWalkDir::new(tmp.path())).await;
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_dir());
    assert_eq!(entries[0].depth(), 0);
}

// ---------------------------------------------------------------------------
// DirEntry::metadata failure — exercises Error::from_entry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_metadata_after_file_removed_follows_platform_caching() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("doomed");
    fs::write(&file, "").unwrap();

    let entry = walk_ok(
        AsyncWalkDir::new(tmp.path())
            .follow_links(true)
            .min_depth(1),
    )
    .await
    .into_iter()
    .find(|e| !e.is_dir())
    .unwrap();

    fs::remove_file(&file).unwrap();

    // Unix leaves a plain file's cell empty, so this first call does the I/O
    // now and fails, exercising the error path. Windows takes its metadata
    // from the directory scan, so the entry keeps the snapshot the walk saw.
    let result = entry.metadata().await;
    #[cfg(unix)]
    {
        assert!(result.is_err());
        assert!(result.unwrap_err().is_io());
    }
    #[cfg(windows)]
    assert!(result.is_ok());
}

// ---------------------------------------------------------------------------
// Debug format — covers AsyncWalkDir::fmt and AsyncWalker::fmt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_walkdir_debug_with_sort_by_contains_sorter() {
    let walker = AsyncWalkDir::new(".").sort_by(|e| async move { e.file_name().to_owned() });
    assert!(format!("{walker:?}").contains("Sorter"));
}

#[tokio::test]
async fn test_async_walker_debug_does_not_panic() {
    let tmp = basic_tree();
    let walker = AsyncWalkDir::new(tmp.path()).walker();
    let _ = format!("{walker:?}");
}

#[tokio::test]
async fn test_async_walker_debug_after_first_entry_does_not_panic() {
    // The walker owns a boxed stream, so its Debug reports the options rather
    // than the traversal stack; it must stay printable mid-walk.
    let tmp = basic_tree();
    let mut walker = AsyncWalkDir::new(tmp.path()).walker();
    let _ = walker.next_entry().await;
    assert!(format!("{walker:?}").contains("AsyncWalker"));
}

// ---------------------------------------------------------------------------
// Platform-specific
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg(unix)]
async fn test_async_unix_direntry_ino_is_nonzero() {
    use std::os::unix::fs::DirEntryExt;

    let tmp = basic_tree();
    let entry = walk_ok(AsyncWalkDir::new(tmp.path()))
        .await
        .into_iter()
        .next()
        .unwrap();
    assert!(entry.ino() > 0);
}

#[tokio::test]
#[cfg(windows)]
async fn test_async_windows_junction_without_follow_links_is_bounded() {
    let Some(tmp) = junction_loop_tree() else {
        return;
    };
    // Bounded so a missed loop fails the test rather than hanging it.
    let (oks, errs): (Vec<_>, Vec<_>) = walk_all(AsyncWalkDir::new(tmp.path()).max_depth(12))
        .await
        .into_iter()
        .partition(Result::is_ok);
    let entries: Vec<_> = oks.into_iter().map(Result::unwrap).collect();
    let errors: Vec<_> = errs.into_iter().map(Result::unwrap_err).collect();

    if entries.iter().any(|e| e.file_name() == "j" && e.is_dir()) {
        // Windows reported the junction as a plain directory, so the walk
        // descended through it and the reparse point had to arm loop
        // detection. This is the branch the arming exists for, and it cannot
        // be forced on NTFS, where a junction reads as a link instead.
        assert!(errors.iter().any(|e| e.is_loop()), "{errors:?}");
    } else {
        // A junction reporting as a link is never descended into, so nothing
        // arms and no handle is opened: a, a/b and a/b/j, and no error.
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(entries.iter().filter(|e| e.depth() > 0).count(), 3);
    }
}

#[tokio::test]
#[cfg(windows)]
async fn test_async_windows_junction_loop_with_follow_links_returns_error() {
    // A junction needs no privileges, so unlike `loop_tree` this reaches loop
    // detection through a reparse point wherever the tests run.
    let Some(tmp) = junction_loop_tree() else {
        return;
    };
    let results = walk_all(
        AsyncWalkDir::new(tmp.path())
            .follow_links(true)
            .max_depth(12),
    )
    .await;
    assert!(results.into_iter().any(|r| r.is_err_and(|e| e.is_loop())));
}
