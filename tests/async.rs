#![cfg(feature = "async")]

#[cfg(unix)]
use std::os::unix::fs as unix_fs;
#[cfg(windows)]
use std::os::windows::fs as win_fs;
use std::{collections::BTreeSet, fs, path::Path};

use tempfile::TempDir;
use walkthrough::{Async, AsyncWalkDir, AsyncWalker, DirEntry, ErrorKind, Result};

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
///   dangling  ->  __no_such_target__
/// ```
fn dangling_symlink_tree() -> Option<TempDir> {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    #[cfg(unix)]
    unix_fs::symlink("__no_such_target__", r.join("dangling")).unwrap();
    #[cfg(windows)]
    win_fs::symlink_dir("__no_such_target__", r.join("dangling")).ok()?;
    Some(tmp)
}

// ---------------------------------------------------------------------------
// Async walk helpers
// ---------------------------------------------------------------------------

async fn walk_all(walk: AsyncWalkDir) -> Vec<Result<DirEntry<Async>>> {
    let mut walker: AsyncWalker = walk.walker().await;
    let mut out = Vec::new();
    while let Some(res) = walker.next().await {
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

// ---------------------------------------------------------------------------
// Sorting — exercises DirStream::Sorted via collect_sorted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_sort_by_name_orders_siblings_alphabetically() {
    let tmp = basic_tree();
    // sort_by forces DirStream::Sorted for every directory level.
    let depth1: Vec<String> =
        walk_ok(AsyncWalkDir::new(tmp.path()).sort_by(|a, b| a.file_name().cmp(b.file_name())))
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
            .sort_by(|a, b| a.file_name().cmp(b.file_name()))
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
async fn test_async_metadata_error_after_file_removed() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("doomed");
    fs::write(&file, "").unwrap();

    // follow_links=true: on Windows metadata() always re-fetches; on unix the
    // OnceCell is populated for dirs/symlinks at construction, but for a plain
    // file with follow_link=true metadata() calls fs::metadata at invocation
    // time, so deleting the file beforehand forces the I/O to fail.
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

    let result = entry.metadata().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().is_io());
}

// ---------------------------------------------------------------------------
// Debug format — covers AsyncWalker::fmt and DirStream::fmt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_walkdir_debug_with_sort_by_contains_sorter() {
    let walker = AsyncWalkDir::new(".").sort_by(|a, b| a.file_name().cmp(b.file_name()));
    assert!(format!("{walker:?}").contains("Sorter"));
}

#[tokio::test]
async fn test_async_walker_debug_does_not_panic() {
    let tmp = basic_tree();
    let walker = AsyncWalkDir::new(tmp.path()).walker().await;
    let _ = format!("{walker:?}");
}

#[tokio::test]
async fn test_async_walker_debug_with_live_stream_does_not_panic() {
    // After calling next() once the stack holds a DirStream::Live item; its
    // Debug impl must not panic.
    let tmp = basic_tree();
    let mut walker = AsyncWalkDir::new(tmp.path()).walker().await;
    let _ = walker.next().await; // root → pushes a Live stream onto the stack
    let _ = format!("{walker:?}");
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
