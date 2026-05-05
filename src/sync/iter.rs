use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{DirEntry, Error, Result, sync::entry::Ancestor};

/// Configuration options for the directory traversal.
///
/// This struct holds the settings that define how the [`WalkDir`] walker
/// behaves, such as depth limits, sorting, and link following.
#[derive(Debug)]
struct WalkDirOptions {
    /// Whether to follow symbolic links.
    follow_links: bool,
    /// The minimum depth to start reporting entries (0 is the root).
    min_depth: usize,
    /// The maximum depth to recurse into.
    max_depth: usize,
    /// Whether to sort directories before files in the output.
    group_dir: bool,
    /// Whether to include hidden files (files starting with a dot).
    skip_hidden: bool,
}

impl Default for WalkDirOptions {
    /// Returns the default configuration:
    /// - `follow_links`: false
    /// - `min_depth`: 0
    /// - `max_depth`: usize::MAX
    /// - `group_dir`: false
    /// - `show_hidden`: false
    /// - `sorter`: None
    fn default() -> Self {
        Self {
            follow_links: false,
            min_depth: 0,
            max_depth: usize::MAX,
            group_dir: false,
            skip_hidden: true,
        }
    }
}

/// A builder for configuring a directory traversal.
///
/// This is the entry point for the library. You can chain methods to
/// configure the walk and finally call [`walker()`](Self::walker) to
/// start the process.
///
/// # Example
/// ```rust
/// let walkdir = WalkDir::new("./my_project")
///     .max_depth(2)
///     .show_hidden(true);
///
/// let mut walker = walkdir.walker();
/// ```
#[derive(Debug)]
pub struct WalkDir {
    root: PathBuf,
    opts: WalkDirOptions,
}

impl WalkDir {
    /// Creates a new [`WalkDir`] builder starting at the given `root` path.
    ///
    /// # Example
    /// ```rust
    /// let walk = WalkDir::new("/home/user/documents");
    /// ```
    pub fn new<P>(root: P) -> Self
    where
        P: AsRef<Path>,
    {
        Self {
            root: root.as_ref().to_path_buf(),
            opts: WalkDirOptions::default(),
        }
    }

    /// Sets the minimum depth of the traversal.
    ///
    /// Entries with a depth less than this will not be yielded.
    /// The root has a depth of 0.
    pub fn min_depth(mut self, depth: usize) -> Self {
        self.opts.min_depth = depth;
        if self.opts.min_depth > self.opts.max_depth {
            self.opts.min_depth = self.opts.max_depth;
        }
        self
    }

    /// Sets the maximum depth of the traversal.
    ///
    /// The walker will not descend into directories deeper than this value.
    /// Use `0` to only visit the root directory.
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.opts.max_depth = depth;
        if self.opts.max_depth < self.opts.min_depth {
            self.opts.max_depth = self.opts.min_depth;
        }
        self
    }

    /// Set to `true` to follow symbolic links.
    ///
    /// **Note:** Enabling this can lead to infinite loops if circular
    /// symlinks exist. The walker includes basic loop detection.
    pub fn follow_links(mut self, yes: bool) -> Self {
        self.opts.follow_links = yes;
        self
    }

    /// If `true`, directories will be yielded before files in the same directory.
    pub fn group_dir(mut self, yes: bool) -> Self {
        self.opts.group_dir = yes;
        self
    }

    /// If `true`, hidden files (those starting with `.`) will be included in the walk.
    pub fn skip_hidden(mut self, yes: bool) -> Self {
        self.opts.skip_hidden = yes;
        self
    }
}

impl IntoIterator for WalkDir {
    type IntoIter = Walker;
    type Item = Result<DirEntry>;

    fn into_iter(self) -> Self::IntoIter {
        Self::IntoIter {
            opts: self.opts,
            stack: vec![WalkTask::new(self.root, 0)],
            ancestors: Vec::new(),
        }
    }
}

#[derive(Debug)]
enum DirState {
    Pending(PathBuf),
    Open(Box<fs::ReadDir>),
}

/// Represents a single directory to be processed by the walker.
///
/// This is used internally to manage the stack of directories yet to be visited.
#[derive(Debug)]
struct WalkTask {
    state: DirState,
    depth: usize,
}

impl WalkTask {
    /// Creates a new task for the given directory reading, depth and ancestor if available.
    fn new(path: PathBuf, depth: usize) -> Self {
        Self {
            state: DirState::Pending(path),
            depth,
        }
    }

    fn open(&mut self) -> Result<()> {
        if let DirState::Pending(path) = &self.state {
            let rd = fs::read_dir(path)
                .map_err(|err| Error::new_io_error(path.to_path_buf(), self.depth(), err))?;
            self.state = DirState::Open(Box::new(rd));
        }
        Ok(())
    }

    /// Returns the depth of this directory relative to the root.
    fn depth(&self) -> usize {
        self.depth
    }
}

/// The stateful engine for the directory traversal.
///
/// Use [`next_node()`](Self::next_node) to asynchronously retrieve
/// the next directory and its entries.
#[derive(Debug)]
pub struct Walker {
    /// Configuration options copied from the builder.
    opts: WalkDirOptions,
    /// The LIFO stack for Depth-First Search (DFS).
    stack: Vec<WalkTask>,
    /// Tracked ancestors to detect infinite loops with symlinks.
    ancestors: Vec<Ancestor>,
}

impl Iterator for Walker {
    type Item = Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(task) = self.stack.last_mut() {
            let depth = task.depth();
            if let Err(err) = task.open() {
                self.stack.pop();
                return Some(Err(err));
            }

            if let DirState::Open(ref mut rd) = task.state {
                match rd.next() {
                    Some(Ok(std_entry)) => {
                        let entry =
                            match DirEntry::from_std(&std_entry, depth + 1, self.opts.follow_links)
                            {
                                Ok(e) => e,
                                Err(err) => return Some(Err(err)),
                            };

                        if entry.depth() < self.opts.min_depth {
                            continue;
                        }

                        if self.opts.skip_hidden && entry.is_hidden() {
                            continue;
                        }

                        // Loop-Detection
                        if let Some(anc) = entry.ancestor() {
                            self.ancestors.truncate(depth);
                            if self.ancestors.iter().any(|a| a == &anc) {
                                return Some(Err(Error::new_loop(
                                    entry.path().to_path_buf(),
                                    entry.depth(),
                                )));
                            }
                        }

                        if entry.is_dir() && entry.depth() < self.opts.max_depth {
                            if let Some(anc) = entry.ancestor() {
                                self.ancestors.push(anc);
                            }
                            self.stack
                                .push(WalkTask::new(entry.path().to_path_buf(), entry.depth()));
                        }
                        return Some(Ok(entry));
                    }
                    Some(Err(err)) => {
                        return Some(Err(Error::new_io_error(PathBuf::new(), depth + 1, err)));
                    }
                    None => {
                        self.stack.pop();
                        continue;
                    }
                }
            }
        }
        None
    }
}
