// The option set both walkers share. Sorting is not part of it: the
// synchronous builder takes a comparator, the asynchronous one an async key.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WalkDirOptions {
    pub(crate) min_depth: usize,
    pub(crate) max_depth: usize,
    pub(crate) follow_links: bool,
    pub(crate) group_dir: bool,
    pub(crate) skip_hidden: bool,
}

impl Default for WalkDirOptions {
    fn default() -> Self {
        Self {
            min_depth: 0,
            max_depth: usize::MAX,
            follow_links: false,
            group_dir: false,
            skip_hidden: false,
        }
    }
}
