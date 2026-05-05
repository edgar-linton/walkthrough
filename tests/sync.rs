#[cfg(unix)]
use std::os::unix::fs as unix_fs;
#[cfg(windows)]
use std::os::windows::fs as win_fs;
use std::{collections::BTreeSet, fs, path::Path};

use tempfile::TempDir;
use walkthrough::WalkDir;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// ```text
/// root/
///   .hidden0a
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
    fs::write(r.join(".hidden0a"), "").unwrap();
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
fn symlink_tree() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    fs::write(r.join("real_file"), "hello").unwrap();
    #[cfg(unix)]
    unix_fs::symlink(r.join("real_file"), r.join("link_to_file")).unwrap();
    #[cfg(windows)]
    win_fs::symlink_file(r.join("real_file"), r.join("link_to_file")).unwrap();
    let real_dir = r.join("real_dir");
    fs::create_dir(&real_dir).unwrap();
    fs::write(real_dir.join("inside"), "").unwrap();
    #[cfg(unix)]
    unix_fs::symlink(&real_dir, r.join("link_to_dir")).unwrap();
    #[cfg(windows)]
    win_fs::symlink_dir(&real_dir, r.join("link_to_dir")).unwrap();
    tmp
}

/// ```text
/// root/
///   a/
///     b/
///       loop  ->  root/a   (symlink cycle)
/// ```
fn loop_tree() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    let a = r.join("a");
    fs::create_dir_all(a.join("b")).unwrap();
    #[cfg(unix)]
    unix_fs::symlink(&a, a.join("b").join("loop")).unwrap();
    #[cfg(windows)]
    win_fs::symlink_dir(&a, a.join("b").join("loop")).unwrap();
    tmp
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
                .into_owned()
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
    let tmp = symlink_tree();
    let paths = rel_paths(tmp.path(), WalkDir::new(tmp.path()));

    // The symlink itself must be listed.
    assert!(paths.contains("link_to_dir"));
    // `inside` is reachable only via real_dir, not via the symlink.
    assert_eq!(paths.iter().filter(|p| p.ends_with("inside")).count(), 1);
}

#[test]
fn test_follow_links_true_descends_into_symlinked_dir() {
    let tmp = symlink_tree();
    let paths = rel_paths(tmp.path(), WalkDir::new(tmp.path()).follow_links(true));

    assert!(paths.contains("link_to_dir"));
    // `inside` now appears once under real_dir and once under link_to_dir.
    assert_eq!(paths.iter().filter(|p| p.ends_with("inside")).count(), 2);
}

#[test]
fn test_follow_links_loop_returns_error() {
    let tmp = loop_tree();
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
