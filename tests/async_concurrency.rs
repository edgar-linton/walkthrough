#![cfg(feature = "async")]

//! Tests for `AsyncWalkDir::concurrency`.
//!
//! Resolving a directory's entries concurrently is a latency change and must
//! not be an ordering change, so most of what is worth asserting here is an
//! equivalence: the same tree walked at different concurrency levels must
//! produce byte-identical sequences. A bug in the bounded fan-out shows up as
//! entries arriving in completion order rather than directory order, which is
//! invisible on a fast local filesystem unless it is asserted directly.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use tempfile::TempDir;
use walkthrough::{AsyncDirEntry, AsyncWalkDir};

/// root/
/// ├── dir_a/         (holding 12 files of descending size)
/// ├── dir_b/
/// │   └── nested/
/// ├── same_0 … same_9  (10 files of identical size — the tie-break case)
/// └── .hidden
fn wide_tree() -> TempDir {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let root = tmp.path();

    fs::create_dir(root.join("dir_a")).unwrap();
    for i in 0..12 {
        fs::write(
            root.join("dir_a").join(format!("f{i:02}")),
            vec![b'x'; 24 - i],
        )
        .unwrap();
    }
    fs::create_dir(root.join("dir_b")).unwrap();
    fs::create_dir(root.join("dir_b").join("nested")).unwrap();
    fs::write(root.join("dir_b").join("nested").join("deep"), b"deep").unwrap();

    // Identical sizes, so only the file-name tie-break can order these.
    for i in 0..10 {
        fs::write(root.join(format!("same_{i}")), b"same").unwrap();
    }
    fs::write(root.join(".hidden"), b"h").unwrap();
    tmp
}

/// Every concurrency level worth distinguishing: the sequential fallback, a
/// level below the directory width so the fan-out has to refill, and a level
/// above it so it does not.
const LEVELS: [usize; 5] = [0, 1, 2, 32, 512];

/// Walks `root` ordered by size, then name, at `concurrency`.
async fn walk_sorted(root: &Path, concurrency: usize, group_dir: bool) -> Vec<PathBuf> {
    let mut walker = AsyncWalkDir::new(root)
        .concurrency(concurrency)
        .group_dir(group_dir)
        .sort_by(|entry: Arc<AsyncDirEntry>| async move {
            entry.metadata().await.map(|m| m.len()).unwrap_or(0)
        })
        .walker()
        .await;

    let mut out = Vec::new();
    while let Some(entry) = walker.next().await {
        out.push(
            entry
                .expect("fixture tree should not produce walk errors")
                .into_path(),
        );
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_order_is_identical_at_every_concurrency() {
    let tmp = wide_tree();
    let expected = walk_sorted(tmp.path(), 1, false).await;
    assert!(
        expected.len() > 20,
        "fixture should be wide enough to matter"
    );

    for level in LEVELS {
        assert_eq!(
            walk_sorted(tmp.path(), level, false).await,
            expected,
            "concurrency {level} changed the order"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_order_is_identical_at_every_concurrency_with_group_dir() {
    let tmp = wide_tree();
    let expected = walk_sorted(tmp.path(), 1, true).await;

    for level in LEVELS {
        assert_eq!(
            walk_sorted(tmp.path(), level, true).await,
            expected,
            "concurrency {level} changed the grouped order"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_order_is_identical_at_every_concurrency_for_the_streaming_path() {
    // No sort_by and no group_dir keeps the walker on its streaming path,
    // which resolves entries in batches of `concurrency`.
    let tmp = wide_tree();

    async fn walk_streaming(root: &Path, concurrency: usize) -> Vec<PathBuf> {
        let mut walker = AsyncWalkDir::new(root)
            .concurrency(concurrency)
            .skip_hidden(true)
            .walker()
            .await;
        let mut out = Vec::new();
        while let Some(entry) = walker.next().await {
            out.push(entry.unwrap().into_path());
        }
        out
    }

    let expected = walk_streaming(tmp.path(), 1).await;
    for level in LEVELS {
        let mut got = walk_streaming(tmp.path(), level).await;
        let mut want = expected.clone();
        // Filesystem order is not reproducible, so compare as sets here; the
        // ordered paths above are what pin the order itself.
        got.sort();
        want.sort();
        assert_eq!(got, want, "concurrency {level} lost or duplicated entries");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_equal_keys_are_ordered_by_name() {
    let tmp = wide_tree();
    let paths = walk_sorted(tmp.path(), 32, false).await;

    // The ten `same_*` files share a size, so only the name tie-break can
    // separate them — and it must, or `offset` has nothing to stand on.
    let same: Vec<String> = paths
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter(|n| n.starts_with("same_"))
        .collect();

    let mut sorted = same.clone();
    sorted.sort();
    assert_eq!(same, sorted, "equal keys are not ordered by name");
    assert_eq!(same.len(), 10);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_order_is_reproducible_across_walks() {
    let tmp = wide_tree();
    let first = walk_sorted(tmp.path(), 32, true).await;
    let second = walk_sorted(tmp.path(), 32, true).await;
    assert_eq!(first, second);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_key_is_awaited_exactly_once_per_entry() {
    let tmp = wide_tree();
    let calls = Arc::new(AtomicUsize::new(0));

    let counter = Arc::clone(&calls);
    let mut walker = AsyncWalkDir::new(tmp.path())
        .concurrency(8)
        .sort_by(move |entry: Arc<AsyncDirEntry>| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                entry.metadata().await.map(|m| m.len()).unwrap_or(0)
            }
        })
        .walker()
        .await;

    // The root is yielded without a key being computed for it, so the key runs
    // once per entry below it.
    let mut entries = 0;
    while let Some(entry) = walker.next().await {
        entry.unwrap();
        entries += 1;
    }

    assert_eq!(calls.load(Ordering::SeqCst), entries - 1);
}

#[tokio::test]
async fn test_concurrency_zero_is_clamped_and_still_walks() {
    let tmp = wide_tree();
    let all = walk_sorted(tmp.path(), 0, false).await;
    let one = walk_sorted(tmp.path(), 1, false).await;
    assert_eq!(all, one);
    assert!(!all.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_skip_hidden_applies_on_the_concurrent_path() {
    let tmp = wide_tree();
    let mut walker = AsyncWalkDir::new(tmp.path())
        .concurrency(16)
        .skip_hidden(true)
        .sort_by(|entry: Arc<AsyncDirEntry>| async move { entry.file_name().to_owned() })
        .walker()
        .await;

    while let Some(entry) = walker.next().await {
        let entry = entry.unwrap();
        assert!(
            entry.depth() == 0 || !entry.file_name().to_string_lossy().starts_with('.'),
            "hidden entry {:?} survived",
            entry.path()
        );
    }
}
