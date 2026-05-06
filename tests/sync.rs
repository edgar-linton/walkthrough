#[cfg(unix)]
use std::os::unix::fs as unix_fs;
#[cfg(windows)]
use std::os::windows::fs as win_fs;
use std::{collections::BTreeSet, fs, path::Path};

use tempfile::TempDir;
use walkthrough::{ErrorKind, WalkDir};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// On Windows, mark a path as hidden by setting FILE_ATTRIBUTE_HIDDEN.
/// On Unix the dot-prefix is sufficient and this is a no-op.
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
/// 8 non-root entries, 9 total (including root at depth 0).
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
/// Returns `None` on Windows when the process lacks symlink privileges.
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
///       loop  ->  root/a   (symlink cycle)
/// ```
/// Returns `None` on Windows when the process lacks symlink privileges.
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
/// Returns `None` on Windows when the process lacks symlink privileges.
fn dangling_symlink_tree() -> Option<TempDir> {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    // On Unix any symlink works; on Windows only a dir symlink causes
    // from_std to call fs::metadata (and fail) when follow_links=true.
    #[cfg(unix)]
    unix_fs::symlink("__no_such_target__", r.join("dangling")).unwrap();
    #[cfg(windows)]
    win_fs::symlink_dir("__no_such_target__", r.join("dangling")).ok()?;
    Some(tmp)
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn rel_paths(root: &Path, walk: WalkDir) -> BTreeSet<String> {
    walk.into_iter()
        .filter_map(|r| r.ok())
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
// Basic traversal
// ---------------------------------------------------------------------------

#[test]
fn test_all_entries_are_visited() {
    let tmp = basic_tree();
    let paths = rel_paths(tmp.path(), WalkDir::new(tmp.path()));
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

#[test]
fn test_root_is_yielded_at_depth_0() {
    let tmp = basic_tree();
    let first = WalkDir::new(tmp.path())
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(first.depth(), 0);
    assert_eq!(first.path(), tmp.path());
}

// ---------------------------------------------------------------------------
// Depth limits
// ---------------------------------------------------------------------------

#[test]
fn test_max_depth_1_does_not_descend_into_subdirs() {
    let tmp = basic_tree();
    let paths = rel_paths(tmp.path(), WalkDir::new(tmp.path()).max_depth(1));
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

#[test]
fn test_max_depth_0_yields_only_root() {
    let tmp = basic_tree();
    let entries: Vec<_> = WalkDir::new(tmp.path())
        .max_depth(0)
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].depth(), 0);
}

#[test]
fn test_min_depth_1_skips_root() {
    let tmp = basic_tree();
    let entries: Vec<_> = WalkDir::new(tmp.path())
        .min_depth(1)
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert!(entries.iter().all(|e| e.depth() >= 1));
    assert_eq!(entries.len(), 8);
}

#[test]
fn test_min_depth_2_skips_depth_0_and_1_entries() {
    let tmp = basic_tree();
    let entries: Vec<_> = WalkDir::new(tmp.path())
        .min_depth(2)
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    // dir2a, file1a_a, file1b_a, file2a_a
    assert!(entries.iter().all(|e| e.depth() >= 2));
    assert_eq!(entries.len(), 4);
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

#[test]
fn test_skip_hidden_excludes_dot_entries() {
    let tmp = basic_tree();
    let paths = rel_paths(tmp.path(), WalkDir::new(tmp.path()).skip_hidden(true));
    assert!(!paths.contains(".hidden0a"));
    assert!(paths.contains("file0a"));
    assert!(paths.contains("dir1a"));
}

// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

#[test]
fn test_sort_by_name_orders_siblings_alphabetically() {
    let tmp = basic_tree();
    // Collect only the depth-1 entries in traversal order.  In a DFS with
    // sorted reads, sibling directories appear in sorted order even though
    // their subtrees are interleaved between them.
    let depth1: Vec<String> = WalkDir::new(tmp.path())
        .sort_by(|a, b| a.file_name().cmp(b.file_name()))
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|e| e.depth() == 1)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    let mut sorted = depth1.clone();
    sorted.sort();
    assert_eq!(depth1, sorted);
}

#[test]
fn test_group_dir_places_dirs_before_files() {
    let tmp = basic_tree();
    let depth1: Vec<_> = WalkDir::new(tmp.path())
        .group_dir(true)
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|e| e.depth() == 1)
        .collect();

    // All directories must appear before any plain file.
    let first_file = depth1.iter().position(|e| !e.is_dir());
    let last_dir = depth1.iter().rposition(|e| e.is_dir());
    if let (Some(f), Some(d)) = (first_file, last_dir) {
        assert!(d < f, "a directory appeared after a file at depth 1");
    }
}

// ---------------------------------------------------------------------------
// Symlink handling
// ---------------------------------------------------------------------------

#[test]
fn test_follow_links_false_does_not_descend_into_symlinked_dir() {
    let Some(tmp) = symlink_tree() else { return };
    let paths = rel_paths(tmp.path(), WalkDir::new(tmp.path()));

    assert!(paths.contains("link_to_dir"));
    assert_eq!(paths.iter().filter(|p| p.ends_with("inside")).count(), 1);
}

#[test]
fn test_follow_links_true_descends_into_symlinked_dir() {
    let Some(tmp) = symlink_tree() else { return };
    let paths = rel_paths(tmp.path(), WalkDir::new(tmp.path()).follow_links(true));

    assert!(paths.contains("link_to_dir"));
    assert_eq!(paths.iter().filter(|p| p.ends_with("inside")).count(), 2);
}

#[test]
fn test_follow_links_loop_returns_error() {
    let Some(tmp) = loop_tree() else { return };
    let result: Vec<_> = WalkDir::new(tmp.path())
        .follow_links(true)
        .into_iter()
        .collect();
    assert!(
        result
            .into_iter()
            .any(|r| r.is_err_and(|err| err.is_loop())),
    );
}

#[test]
fn test_follow_links_dangling_symlink_yields_io_error() {
    let Some(tmp) = dangling_symlink_tree() else {
        return;
    };
    let errors: Vec<_> = WalkDir::new(tmp.path())
        .follow_links(true)
        .into_iter()
        .filter_map(|r| r.err())
        .collect();
    assert!(!errors.is_empty());
    assert!(errors.iter().all(|e| e.is_io()));
}

// ---------------------------------------------------------------------------
// Error API
// ---------------------------------------------------------------------------

#[test]
fn test_io_error_exposes_all_accessors() {
    let result: Vec<_> = WalkDir::new("__nonexistent_walkthrough_xyz__")
        .into_iter()
        .collect();
    assert_eq!(result.len(), 1);
    let err = result.into_iter().next().unwrap().unwrap_err();

    assert!(err.is_io());
    assert!(!err.is_loop());
    assert_eq!(err.depth(), 0);
    assert!(err.path().ends_with("__nonexistent_walkthrough_xyz__"));
    assert!(matches!(err.kind(), ErrorKind::Io(_)));
    assert!(err.io_error().is_some());
    assert!(!err.to_string().is_empty());
}

#[test]
fn test_io_error_into_io_error_is_some() {
    let err = WalkDir::new("__nonexistent_walkthrough_abc__")
        .into_iter()
        .next()
        .unwrap()
        .unwrap_err();
    assert!(err.into_io_error().is_some());
}

#[test]
fn test_loop_error_exposes_all_accessors() {
    let Some(tmp) = loop_tree() else { return };
    let err = WalkDir::new(tmp.path())
        .follow_links(true)
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

#[test]
fn test_loop_error_into_io_error_is_none() {
    let Some(tmp) = loop_tree() else { return };
    let err = WalkDir::new(tmp.path())
        .follow_links(true)
        .into_iter()
        .find_map(|r| r.err().filter(|e| e.is_loop()))
        .unwrap();
    assert!(err.into_io_error().is_none());
}

// ---------------------------------------------------------------------------
// DirEntry API
// ---------------------------------------------------------------------------

#[test]
fn test_direntry_into_path_matches_path() {
    let tmp = basic_tree();
    let entry = WalkDir::new(tmp.path())
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    let expected = entry.path().to_path_buf();
    assert_eq!(entry.into_path(), expected);
}

#[test]
fn test_direntry_file_type_distinguishes_dirs_and_files() {
    let tmp = basic_tree();
    let entries: Vec<_> = WalkDir::new(tmp.path())
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();

    let root = entries.iter().find(|e| e.depth() == 0).unwrap();
    assert!(root.file_type().is_dir());

    let file = entries
        .iter()
        .find(|e| e.path().ends_with("file0a"))
        .unwrap();
    assert!(file.file_type().is_file());
}

#[test]
fn test_direntry_metadata_is_ok_for_all_entries() {
    let tmp = basic_tree();
    // follow_links(true) exercises the `follow_link = true` branch of metadata()
    // for every non-symlink entry (regular files and directories).
    for entry in WalkDir::new(tmp.path())
        .follow_links(true)
        .into_iter()
        .filter_map(|r| r.ok())
    {
        assert!(entry.metadata().is_ok());
    }
}

// ---------------------------------------------------------------------------
// Walker edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_nonexistent_root_yields_single_io_error() {
    let result: Vec<_> = WalkDir::new("__nonexistent_walkthrough_root__")
        .into_iter()
        .collect();
    assert_eq!(result.len(), 1);
    assert!(result[0].as_ref().unwrap_err().is_io());
}

#[test]
fn test_file_as_root_yields_single_non_dir_entry() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("lone_file");
    fs::write(&file, "").unwrap();

    let entries: Vec<_> = WalkDir::new(&file)
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].is_dir());
    assert_eq!(entries[0].depth(), 0);
}

#[test]
fn test_empty_directory_yields_only_root() {
    let tmp = TempDir::new().unwrap();
    let entries: Vec<_> = WalkDir::new(tmp.path())
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_dir());
    assert_eq!(entries[0].depth(), 0);
}

// ---------------------------------------------------------------------------
// Debug format (covers Sorter::fmt and Walker::fmt)
// ---------------------------------------------------------------------------

#[test]
fn test_walkdir_debug_with_sort_by_contains_sorter() {
    let walker = WalkDir::new(".").sort_by(|a, b| a.file_name().cmp(b.file_name()));
    assert!(format!("{walker:?}").contains("Sorter"));
}

#[test]
fn test_walker_debug_does_not_panic() {
    let tmp = basic_tree();
    let walker = WalkDir::new(tmp.path()).into_iter();
    let _ = format!("{walker:?}");
}

// ---------------------------------------------------------------------------
// DirEntry::metadata failure and Error::from_entry
// ---------------------------------------------------------------------------

#[test]
fn test_metadata_error_after_file_removed() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("doomed");
    fs::write(&file, "").unwrap();

    // Walk with follow_links=true so every DirEntry has follow_link=true.
    // For a regular file on Unix, the metadata OnceCell is NOT pre-populated,
    // so metadata() calls fs::metadata at call-time.
    // On Windows, metadata() always re-calls fs::metadata when follow_link=true.
    // Either way, deleting the file before calling metadata() forces that I/O
    // call to fail, which exercises Error::from_entry.
    let entry = WalkDir::new(tmp.path())
        .follow_links(true)
        .min_depth(1)
        .into_iter()
        .filter_map(|r| r.ok())
        .find(|e| !e.is_dir())
        .unwrap();

    fs::remove_file(&file).unwrap();

    let result = entry.metadata();
    assert!(result.is_err());
    assert!(result.unwrap_err().is_io());
}

// ---------------------------------------------------------------------------
// Platform-specific
// ---------------------------------------------------------------------------

#[test]
#[cfg(unix)]
fn test_unix_direntry_ino_is_nonzero() {
    use std::os::unix::fs::DirEntryExt;

    let tmp = basic_tree();
    let entry = WalkDir::new(tmp.path())
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    assert!(entry.ino() > 0);
}
