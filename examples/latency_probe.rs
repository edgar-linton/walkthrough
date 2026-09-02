//! Measures where the time goes when listing one directory.
//!
//! Run it against the real directory, not a temporary one — the whole point is
//! to find out whether per-entry latency is in the filesystem, which is what
//! `concurrency` can fix:
//!
//! ```text
//! cargo run --release --features async --example latency_probe -- /mnt/share
//! ```
//!
//! Read it as follows:
//!
//! - `names+stat` in the milliseconds means the filesystem is not the
//!   bottleneck, and `concurrency` will not move a slow listing. Look at what
//!   the caller does per entry instead.
//! - `names+stat` in the seconds means one `stat` costs milliseconds — a
//!   network mount or a FUSE layer — and the concurrency rows below show what
//!   overlapping them recovers.
//! - The `concurrency 1` row is what the walker did before 0.4.0.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use walkthrough::r#async::{AsyncDirEntry, AsyncWalkDir};

/// Enumeration alone: one `getdents` batch per 32 entries, no `stat`.
async fn names_only(root: &Path) -> (usize, Duration) {
    let start = Instant::now();
    let mut rd = tokio::fs::read_dir(root).await.expect("read_dir failed");
    let mut entries = 0;
    while rd.next_entry().await.expect("next_entry failed").is_some() {
        entries += 1;
    }
    (entries, start.elapsed())
}

/// Enumeration plus one sequential `stat` per entry — the shape of the walk
/// before per-entry I/O was overlapped.
async fn names_and_stat(root: &Path) -> Duration {
    let start = Instant::now();
    let mut rd = tokio::fs::read_dir(root).await.expect("read_dir failed");
    while let Some(entry) = rd.next_entry().await.expect("next_entry failed") {
        let _ = entry.metadata().await;
    }
    start.elapsed()
}

/// One page of a size-ordered listing, at a given concurrency.
async fn listing(root: &Path, concurrency: usize, limit: usize) -> (usize, Duration) {
    let start = Instant::now();
    let mut walker = AsyncWalkDir::new(root)
        .min_depth(1)
        .max_depth(1)
        .concurrency(concurrency)
        .limit(limit)
        .sort_by(|entry: Arc<AsyncDirEntry>| async move {
            entry.metadata().await.map(|m| m.len()).unwrap_or(0)
        })
        .walker()
        .await;

    let mut entries = 0;
    while let Some(entry) = walker.next().await {
        if entry.is_err() {
            continue;
        }
        entries += 1;
    }
    (entries, start.elapsed())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let Some(root) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: latency_probe <directory>");
        std::process::exit(2);
    };

    let (entries, names) = names_only(&root).await;
    let both = names_and_stat(&root).await;
    let per_entry = both.checked_div(entries.max(1) as u32).unwrap_or_default();

    println!("{}: {entries} entries", root.display());
    println!("  names only                {names:>10.1?}");
    println!("  names + stat, sequential  {both:>10.1?}  ({per_entry:.1?}/entry)");
    println!("  ordered by size, one page of 100:");

    for concurrency in [1usize, 8, 32, 128, 512] {
        let (yielded, elapsed) = listing(&root, concurrency, 100).await;
        println!("    concurrency {concurrency:4}  {elapsed:>10.1?}  ({yielded} yielded)");
    }
}
