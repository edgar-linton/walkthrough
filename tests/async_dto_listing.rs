//! Integration test for a realistic web-handler-style use of `AsyncWalkDir`:
//! walk a directory, project each entry into a plain DTO, sort, and filter —
//! the shape of code you'd put behind an actix-web (or any tokio-based)
//! endpoint that lists a directory tree.
//!
//! This also answers two questions that come up when wiring this into such a
//! handler:
//!
//! - Does `sort_by`'s async key actually avoid blocking the runtime? Yes —
//!   `AsyncDirEntry::metadata()` is backed by `tokio::fs`, which offloads the
//!   syscall to the blocking thread pool rather than blocking the reactor.
//!   The panic scenario ("Cannot start a runtime from within a runtime")
//!   only happens if a *synchronous* comparator tries to bridge back into
//!   async — e.g. by calling `futures::executor::block_on` from inside an
//!   already-running runtime. `AsyncWalkDir::sort_by` takes an async key
//!   specifically so callers never need to do that.
//! - Is the walker's `sort_by` even necessary if you're collecting into a
//!   DTO anyway? Only if per-directory (sibling) ordering must be preserved
//!   in the tree-walk itself. If you just want a flat, top-level sort of the
//!   results, collect first and sort the plain `Vec<Dto>` afterwards — see
//!   `test_flat_sort_after_collect_does_not_need_an_async_key` below.

use std::{fs, path::Path};

use tempfile::TempDir;
use walkthrough::r#async::{AsyncDirEntry, AsyncWalkDir};

/// On Windows, mark a path as hidden by setting FILE_ATTRIBUTE_HIDDEN. On
/// Unix the dot-prefix is sufficient and this is a no-op — mirrors the
/// helper in tests/sync.rs and tests/async.rs.
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

/// Plain data carried across an API boundary — no borrow on the walker, no
/// futures, nothing async left once it's built.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DirEntryDto {
    name: String,
    depth: usize,
    is_dir: bool,
    size: u64,
}

impl DirEntryDto {
    async fn from_entry(entry: &AsyncDirEntry) -> Self {
        let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
        Self {
            name: entry.file_name().to_string_lossy().into_owned(),
            depth: entry.depth(),
            is_dir: entry.is_dir(),
            size,
        }
    }
}

/// Walks `root`, grouping directories first and ordering files by ascending
/// size (ties broken by name) within each directory, optionally keeping only
/// files whose extension matches `extension`.
async fn list_dir(root: &Path, extension: Option<&str>) -> Vec<DirEntryDto> {
    let mut walker = AsyncWalkDir::new(root)
        .skip_hidden(true)
        .sort_by(|entry| async move {
            let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
            (!entry.is_dir(), size, entry.file_name().to_owned())
        })
        .walker();

    let mut out = Vec::new();
    while let Some(res) = walker.next_entry().await {
        let entry = res.expect("fixture tree should not produce walk errors");
        if let Some(ext) = extension
            && !entry.is_dir()
            && entry.path().extension().and_then(|e| e.to_str()) != Some(ext)
        {
            continue;
        }
        out.push(DirEntryDto::from_entry(&entry).await);
    }
    out
}

fn names_at_depth(dtos: &[DirEntryDto], depth: usize) -> Vec<&str> {
    dtos.iter()
        .filter(|d| d.depth == depth)
        .map(|d| d.name.as_str())
        .collect()
}

/// root/
/// ├── a.txt        (10 bytes)
/// ├── b.log        (3 bytes)
/// ├── .hidden      (1 byte)
/// └── sub/
///     └── nested.txt (5 bytes)
fn fixture_tree() -> TempDir {
    let dir = TempDir::new().expect("failed to create temp dir");
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("a.txt"), b"0123456789").unwrap();
    fs::write(dir.path().join("b.log"), b"abc").unwrap();
    let hidden = dir.path().join(".hidden");
    fs::write(&hidden, b"x").unwrap();
    set_hidden(&hidden);
    fs::write(dir.path().join("sub").join("nested.txt"), b"hello").unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_dto_listing_is_sorted_and_grouped_on_a_multi_thread_runtime() {
    let dir = fixture_tree();
    let root = dir.path().to_path_buf();

    // actix-web (like most tokio-based web frameworks) drives handlers on a
    // multi-threaded runtime and runs the work as a spawned task; reproduce
    // that shape here rather than calling list_dir() inline, so a comparator
    // that blocked the runtime would show up as a stall or panic instead of
    // silently working just because it's the test's own top-level task.
    let dtos = tokio::spawn(async move { list_dir(&root, None).await })
        .await
        .expect("walk task panicked");

    // Groups: directories before files, at every depth.
    assert_eq!(names_at_depth(&dtos, 1), vec!["sub", "b.log", "a.txt"]);

    // Sizes confirm the ordering came from awaited metadata, not just name/type.
    let file_sizes: Vec<(&str, u64)> = dtos
        .iter()
        .filter(|d| !d.is_dir)
        .map(|d| (d.name.as_str(), d.size))
        .collect();
    assert_eq!(
        file_sizes,
        vec![("nested.txt", 5), ("b.log", 3), ("a.txt", 10)]
    );

    // Hidden entry excluded by skip_hidden (root itself is exempt from this
    // filter, but ".hidden" is a child and must not appear).
    assert!(!dtos.iter().any(|d| d.name == ".hidden"));
}

#[tokio::test]
async fn test_dto_listing_filters_by_extension() {
    let dir = fixture_tree();
    let dtos = list_dir(dir.path(), Some("txt")).await;

    let files: Vec<&str> = dtos
        .iter()
        .filter(|d| !d.is_dir)
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(files, vec!["nested.txt", "a.txt"]);

    // Directories are never dropped by the extension check, so the walk can
    // still descend into them even though "sub" itself has no extension.
    assert!(dtos.iter().any(|d| d.name == "sub" && d.is_dir));
}

#[tokio::test]
async fn test_dto_listing_skips_hidden_entries() {
    let dir = fixture_tree();
    let dtos = list_dir(dir.path(), None).await;
    // skip_hidden only filters descendants, not the root itself — and
    // tempfile's own directories are dot-prefixed on Linux — so depth 0 is
    // excluded from this check on purpose.
    assert!(!dtos.iter().any(|d| d.depth > 0 && d.name.starts_with('.')));
}

/// Demonstrates the alternative to `AsyncWalkDir::sort_by`: if per-directory
/// ordering during the walk doesn't matter — only a flat, top-level order of
/// the final result does — there's nothing async left to do once every entry
/// is a `DirEntryDto`. A plain synchronous `Vec::sort_by` is enough, and it's
/// simpler than wiring an async key into the walker at all.
#[tokio::test]
async fn test_flat_sort_after_collect_does_not_need_an_async_key() {
    let dir = fixture_tree();

    // No sort_by on the walker — default (unsorted) traversal order.
    let mut walker = AsyncWalkDir::new(dir.path())
        .skip_hidden(true)
        .min_depth(1)
        .walker();

    let mut dtos = Vec::new();
    while let Some(res) = walker.next_entry().await {
        let entry = res.unwrap();
        dtos.push(DirEntryDto::from_entry(&entry).await);
    }
    dtos.retain(|d| !d.is_dir);

    // Sort the finished DTOs directly — synchronous, no runtime involved.
    dtos.sort_by_key(|d| d.size);

    let sizes: Vec<u64> = dtos.iter().map(|d| d.size).collect();
    assert_eq!(sizes, vec![3, 5, 10]);
}
