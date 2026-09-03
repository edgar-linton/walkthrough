use std::{collections::BTreeSet, fs, path::Path, sync::Arc};

use futures::StreamExt;
use tempfile::TempDir;
use walkthrough::r#async::{AsyncWalkDir, Filtering};

// ---------------------------------------------------------------------------
// Fixture
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
///   .hidden          (hidden on all platforms)
///   keep/
///     kept           (3 bytes)
///   prune/
///     pruned         (7 bytes)
///   file0            (1 byte)
/// ```
fn tree() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let r = tmp.path();
    fs::write(r.join("file0"), "x").unwrap();
    let hidden = r.join(".hidden");
    fs::write(&hidden, "").unwrap();
    set_hidden(&hidden);
    fs::create_dir(r.join("keep")).unwrap();
    fs::write(r.join("keep").join("kept"), "abc").unwrap();
    fs::create_dir(r.join("prune")).unwrap();
    fs::write(r.join("prune").join("pruned"), "abcdefg").unwrap();
    tmp
}

async fn rel_paths(root: &Path, walk: AsyncWalkDir) -> BTreeSet<String> {
    let mut walker = walk.walker();
    let mut out = BTreeSet::new();
    while let Some(entry) = walker.next_entry().await {
        let entry = entry.unwrap();
        let rel = entry.path().strip_prefix(root).unwrap().to_path_buf();
        out.insert(rel.to_string_lossy().replace('\\', "/"));
    }
    out
}

// ---------------------------------------------------------------------------
// IgnoreDir prunes, IgnoreEntry does not
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ignore_dir_prunes_the_subtree() {
    let tmp = tree();
    let got = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path()).filter_entry(|e| async move {
            if e.file_name() == "prune" {
                Filtering::IgnoreDir
            } else {
                Filtering::Continue
            }
        }),
    )
    .await;

    assert!(!got.contains("prune"), "the directory itself is dropped");
    assert!(
        !got.contains("prune/pruned"),
        "its subtree is never read: {got:?}"
    );
    assert!(got.contains("keep/kept"), "siblings are unaffected");
}

#[tokio::test]
async fn test_ignore_entry_on_a_dir_drops_it_but_keeps_its_children() {
    let tmp = tree();
    let got = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path()).filter_entry(|e| async move {
            if e.is_dir() {
                Filtering::IgnoreEntry
            } else {
                Filtering::Continue
            }
        }),
    )
    .await;

    assert!(!got.contains("keep"), "the directory is not yielded");
    assert!(!got.contains("prune"), "the directory is not yielded");
    assert!(
        got.contains("keep/kept") && got.contains("prune/pruned"),
        "but it is still descended into: {got:?}"
    );
}

#[tokio::test]
async fn test_ignore_entry_on_a_file_drops_it() {
    let tmp = tree();
    let got = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path()).filter_entry(|e| async move {
            if e.file_name() == "file0" {
                Filtering::IgnoreEntry
            } else {
                Filtering::Continue
            }
        }),
    )
    .await;

    assert!(!got.contains("file0"));
    assert!(got.contains("keep/kept"));
}

// ---------------------------------------------------------------------------
// The predicate is async, and the root is exempt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_filter_may_await_metadata() {
    let tmp = tree();
    let got = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path()).filter_entry(|e| async move {
            if e.is_dir() {
                return Filtering::Continue;
            }
            // Only files of 3 bytes or fewer survive.
            let len = e.metadata().await.map(|m| m.len()).unwrap_or(0);
            if len <= 3 {
                Filtering::Continue
            } else {
                Filtering::IgnoreEntry
            }
        }),
    )
    .await;

    assert!(got.contains("file0"), "1 byte");
    assert!(got.contains("keep/kept"), "3 bytes");
    assert!(!got.contains("prune/pruned"), "7 bytes: {got:?}");
}

#[tokio::test]
async fn test_root_is_exempt_from_the_filter() {
    let tmp = tree();
    // A predicate that rejects everything still yields the root.
    let got = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path()).filter_entry(|_| async move { Filtering::IgnoreDir }),
    )
    .await;

    assert_eq!(got, BTreeSet::from(["".to_string()]));
}

#[tokio::test]
async fn test_continue_everywhere_matches_an_unfiltered_walk() {
    let tmp = tree();
    let unfiltered = rel_paths(tmp.path(), AsyncWalkDir::new(tmp.path())).await;
    let filtered = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path()).filter_entry(|_| async move { Filtering::Continue }),
    )
    .await;

    assert_eq!(unfiltered, filtered);
}

#[tokio::test]
async fn test_last_filter_call_wins() {
    let tmp = tree();
    let got = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path())
            .filter_entry(|_| async move { Filtering::IgnoreDir })
            .filter_entry(|_| async move { Filtering::Continue }),
    )
    .await;

    assert!(got.contains("prune/pruned"), "the second filter applies");
}

// ---------------------------------------------------------------------------
// Composition with the other options
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_filter_composes_with_skip_hidden() {
    let tmp = tree();
    let got = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path())
            .skip_hidden(true)
            .filter_entry(|e| async move {
                if e.file_name() == "prune" {
                    Filtering::IgnoreDir
                } else {
                    Filtering::Continue
                }
            }),
    )
    .await;

    assert!(!got.contains(".hidden"), "skip_hidden still applies");
    assert!(!got.contains("prune"), "the filter still applies");
    assert!(got.contains("keep/kept"));
}

#[tokio::test]
async fn test_filter_composes_with_sort_by() {
    let tmp = tree();
    let mut walker = AsyncWalkDir::new(tmp.path())
        .sort_by(|e| async move { e.file_name().to_owned() })
        .filter_entry(|e| async move {
            if e.file_name() == "prune" {
                Filtering::IgnoreDir
            } else {
                Filtering::Continue
            }
        })
        .walker();

    let mut names = Vec::new();
    while let Some(entry) = walker.next_entry().await {
        names.push(entry.unwrap().file_name().to_string_lossy().into_owned());
    }

    // Root first, then the root's children in name order, with `prune` gone.
    assert!(!names.contains(&"prune".to_string()));
    let root_children: Vec<&String> = names
        .iter()
        .filter(|n| ["file0", "keep", ".hidden"].contains(&n.as_str()))
        .collect();
    let mut sorted = root_children.clone();
    sorted.sort();
    assert_eq!(root_children, sorted, "the sort order still holds");
}

#[tokio::test]
async fn test_filter_is_unaffected_by_concurrency() {
    let tmp = tree();
    let mut first: Option<BTreeSet<String>> = None;
    for concurrency in [1, 2, 8, 64] {
        let got = rel_paths(
            tmp.path(),
            AsyncWalkDir::new(tmp.path())
                .concurrency(concurrency)
                .filter_entry(|e| async move {
                    if e.file_name() == "prune" {
                        Filtering::IgnoreDir
                    } else {
                        Filtering::Continue
                    }
                }),
        )
        .await;
        match &first {
            None => first = Some(got),
            Some(expected) => assert_eq!(*expected, got, "concurrency {concurrency}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The Stream impl, and how it differs from filter_entry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stream_ext_filter_does_not_prune_descent() {
    let tmp = tree();

    // filter_entry prunes: nothing under `prune` is ever read.
    let pruned = rel_paths(
        tmp.path(),
        AsyncWalkDir::new(tmp.path()).filter_entry(|e| async move {
            if e.file_name() == "prune" {
                Filtering::IgnoreDir
            } else {
                Filtering::Continue
            }
        }),
    )
    .await;
    assert!(!pruned.contains("prune/pruned"));

    // StreamExt::filter only drops output, so the subtree still appears.
    let root = tmp.path().to_path_buf();
    let downstream: BTreeSet<String> = AsyncWalkDir::new(tmp.path())
        .walker()
        .filter(|entry| {
            let keep = entry
                .as_ref()
                .map(|e| e.file_name() != "prune")
                .unwrap_or(true);
            async move { keep }
        })
        .map(|entry| {
            let e = entry.unwrap();
            e.path()
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
        .await;

    assert!(!downstream.contains("prune"), "the entry itself is dropped");
    assert!(
        downstream.contains("prune/pruned"),
        "but its subtree was still walked: {downstream:?}"
    );
}

#[tokio::test]
async fn test_stream_skip_take_matches_offset_and_limit() {
    let tmp = tree();
    let key = |e: Arc<walkthrough::r#async::AsyncDirEntry>| async move { e.path().to_path_buf() };

    let paged: Vec<String> = {
        let mut walker = AsyncWalkDir::new(tmp.path())
            .sort_by(key)
            .offset(2)
            .limit(3)
            .walker();
        let mut out = Vec::new();
        while let Some(entry) = walker.next_entry().await {
            out.push(entry.unwrap().path().to_string_lossy().into_owned());
        }
        out
    };

    let combinators: Vec<String> = AsyncWalkDir::new(tmp.path())
        .sort_by(key)
        .walker()
        .skip(2)
        .take(3)
        .map(|entry| entry.unwrap().path().to_string_lossy().into_owned())
        .collect()
        .await;

    assert_eq!(paged, combinators);
}
