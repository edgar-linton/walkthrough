//! Tests for `AsyncWalkDir::limit` and `AsyncWalkDir::offset`.
//!
//! The contract is `skip(offset).take(limit)` over the walk the same builder
//! would otherwise produce, which is what makes the pair interchangeable with
//! the `OFFSET`/`LIMIT` of an SQL-backed listing endpoint. Nearly every test
//! here is that equivalence stated against a different option combination,
//! since the failure mode — a page that quietly drops or repeats an entry at
//! its boundary — is invisible in a single page.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use tempfile::TempDir;
use walkthrough::{AsyncDirEntry, AsyncWalkDir};

/// root/
/// ├── a/
/// │   ├── a1  (3 bytes)
/// │   └── a2  (2 bytes)
/// ├── b/
/// │   └── b1  (1 byte)
/// ├── c/      (empty)
/// ├── f1      (5 bytes)
/// ├── f2      (4 bytes)
/// └── f3      (6 bytes)
fn tree() -> TempDir {
    let tmp = TempDir::new().expect("failed to create temp dir");
    let root = tmp.path();
    fs::create_dir(root.join("a")).unwrap();
    fs::write(root.join("a").join("a1"), b"aaa").unwrap();
    fs::write(root.join("a").join("a2"), b"aa").unwrap();
    fs::create_dir(root.join("b")).unwrap();
    fs::write(root.join("b").join("b1"), b"b").unwrap();
    fs::create_dir(root.join("c")).unwrap();
    fs::write(root.join("f1"), b"fffff").unwrap();
    fs::write(root.join("f2"), b"ffff").unwrap();
    fs::write(root.join("f3"), b"ffffff").unwrap();
    tmp
}

/// The option combinations the equivalence has to hold for. Each returns a
/// builder so a test can page it and walk it whole from the same description.
const COMBINATIONS: [&str; 6] = [
    "name",
    "size",
    "group_dir",
    "group_dir+size",
    "min_depth_1",
    "max_depth_1",
];

fn builder(root: &Path, combination: &str) -> AsyncWalkDir {
    let walk = AsyncWalkDir::new(root);
    match combination {
        "name" => walk.sort_by(|e: Arc<AsyncDirEntry>| async move { e.file_name().to_owned() }),
        "size" => walk.sort_by(|e: Arc<AsyncDirEntry>| async move {
            (
                e.metadata().await.map(|m| m.len()).unwrap_or(0),
                e.file_name().to_owned(),
            )
        }),
        "group_dir" => walk.group_dir(true),
        "group_dir+size" => walk
            .group_dir(true)
            .sort_by(|e: Arc<AsyncDirEntry>| async move {
                (
                    e.metadata().await.map(|m| m.len()).unwrap_or(0),
                    e.file_name().to_owned(),
                )
            }),
        "min_depth_1" => walk
            .min_depth(1)
            .sort_by(|e: Arc<AsyncDirEntry>| async move { e.file_name().to_owned() }),
        "max_depth_1" => walk
            .max_depth(1)
            .sort_by(|e: Arc<AsyncDirEntry>| async move { e.file_name().to_owned() }),
        other => panic!("unknown combination {other}"),
    }
}

async fn collect(walk: AsyncWalkDir) -> Vec<PathBuf> {
    let mut walker = walk.walker();
    let mut out = Vec::new();
    while let Some(entry) = walker.next_entry().await {
        out.push(
            entry
                .expect("fixture tree should not produce walk errors")
                .into_path(),
        );
    }
    out
}

#[tokio::test]
async fn test_limit_takes_a_prefix_of_the_walk() {
    let tmp = tree();
    let all = collect(builder(tmp.path(), "name")).await;

    for limit in 0..=all.len() + 2 {
        let page = collect(builder(tmp.path(), "name").limit(limit)).await;
        assert_eq!(page, all[..limit.min(all.len())], "limit {limit}");
    }
}

#[tokio::test]
async fn test_offset_and_limit_equal_skip_and_take() {
    let tmp = tree();

    for combination in COMBINATIONS {
        let all = collect(builder(tmp.path(), combination)).await;

        for offset in 0..=all.len() {
            for limit in 0..=3 {
                let page =
                    collect(builder(tmp.path(), combination).offset(offset).limit(limit)).await;
                let want: Vec<PathBuf> = all.iter().skip(offset).take(limit).cloned().collect();
                assert_eq!(page, want, "{combination}: offset {offset}, limit {limit}");
            }
        }
    }
}

#[tokio::test]
async fn test_pages_concatenate_to_the_whole_walk() {
    let tmp = tree();

    for combination in COMBINATIONS {
        let all = collect(builder(tmp.path(), combination)).await;

        for size in [1usize, 2, 3, 7] {
            let mut concatenated = Vec::new();
            let mut offset = 0;
            loop {
                let page =
                    collect(builder(tmp.path(), combination).offset(offset).limit(size)).await;
                if page.is_empty() {
                    break;
                }
                offset += page.len();
                concatenated.extend(page);
            }
            assert_eq!(
                concatenated, all,
                "{combination}: pages of {size} do not reassemble the walk"
            );
        }
    }
}

#[tokio::test]
async fn test_offset_past_the_end_yields_nothing_and_is_not_an_error() {
    let tmp = tree();
    let all = collect(builder(tmp.path(), "name")).await;

    let mut walker = builder(tmp.path(), "name").offset(all.len() + 100).walker();
    assert!(walker.next_entry().await.is_none());
}

#[tokio::test]
async fn test_limit_zero_yields_nothing() {
    let tmp = tree();
    let mut walker = builder(tmp.path(), "name").limit(0).walker();
    assert!(walker.next_entry().await.is_none());
}

#[tokio::test]
async fn test_a_skipped_directory_is_still_descended_into() {
    let tmp = tree();
    let all = collect(builder(tmp.path(), "name")).await;

    // `a` sorts early, so an offset past it must not take its children with
    // it — a page is a window over the traversal, not a truncation of it.
    let a_index = all
        .iter()
        .position(|p| p.file_name().unwrap() == "a")
        .expect("fixture contains a directory named a");

    let page = collect(builder(tmp.path(), "name").offset(a_index + 1).limit(2)).await;
    let want: Vec<PathBuf> = all.iter().skip(a_index + 1).take(2).cloned().collect();
    assert_eq!(page, want);
    assert!(
        want.iter().any(|p| p.ends_with("a1")),
        "the skipped directory's children should follow it"
    );
}

#[tokio::test]
async fn test_limit_stops_descending_once_satisfied() {
    let tmp = tree();

    // max_depth is unbounded and the root sorts first, so a limit of 1 must
    // return the root and nothing else, having opened it but read no further.
    let page = collect(builder(tmp.path(), "name").limit(1)).await;
    assert_eq!(page.len(), 1);
    assert_eq!(page[0], tmp.path());
}

#[tokio::test]
async fn test_pagination_composes_with_concurrency() {
    let tmp = tree();
    let all = collect(builder(tmp.path(), "size")).await;

    for level in [1usize, 2, 64] {
        let page = collect(
            builder(tmp.path(), "size")
                .concurrency(level)
                .offset(2)
                .limit(3),
        )
        .await;
        let want: Vec<PathBuf> = all.iter().skip(2).take(3).cloned().collect();
        assert_eq!(page, want, "concurrency {level}");
    }
}
