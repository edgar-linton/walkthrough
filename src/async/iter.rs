impl Walker {
    /// Fetches the next directory node from the traversal.
    ///
    /// This method is asynchronous and returns:
    ///
    /// - `Ok(Some(DirNode))`: Successfully read a directory.
    /// - `Ok(None)`: Traversal is complete.
    /// - `Err(WalkDirError)`: An error occurred (e.g., permission denied).
    ///
    /// # Example
    /// ```no_run
    /// use dmp_fs::walkdir::WalkDir;
    ///
    /// let mut walker = WalkDir::new("src").walker();
    ///
    /// while let Some(node) = walker.next_node().await.transpose() {
    ///     let node = node?;
    ///     println!("Directory: {:?}", node.path());
    /// }
    /// ```
    pub async fn next_node(&mut self) -> Result<Option<DirNode>, WalkDirError> {
        // Pop tasks from the stack until we find one within the depth limit
        let task = loop {
            let task = match self.stack.pop() {
                Some(t) => t,
                None => return Ok(None),
            };
            if task.depth() <= self.opts.max_depth {
                break task;
            }
        };
        let depth = task.depth();

        // Update ancestor tracking to current depth
        self.ancestors.truncate(task.depth());
        if let Some(ancestor) = task.ancestor() {
            if self.ancestors.iter().any(|a| a == ancestor) {
                return Err(WalkDirError::from_loop(task.path().to_path_buf(), depth));
            }
            self.ancestors.push(ancestor.clone());
        }

        // Asynchronously read the contents of the directory
        let mut read_dir = fs::read_dir(task.path())
            .await
            .map_err(|err| WalkDirError::from_io(task.path().to_path_buf(), task.depth(), err))?;

        let mut dir_entries = Vec::new();
        let follow_link = self.opts.follow_links;

        // Iterate through entries in the current directory
        while let Some(entry_res) = read_dir.next_entry().await.transpose() {
            let entry = match entry_res {
                Ok(raw_entry) => {
                    // Filter hidden files if not requested
                    if !self.opts.show_hidden
                        && raw_entry.file_name().to_string_lossy().starts_with(".")
                    {
                        continue;
                    }

                    let entry_res = DirEntry::from_entry(raw_entry, follow_link).await;

                    // If entry is a directory, push it onto the stack for later processing
                    if let Ok(entry) = &entry_res
                        && entry.is_dir()
                        && depth < self.opts.max_depth
                    {
                        let ancestor = Ancestor::new(&entry);
                        let new_task = WalkTask::new(entry.path().to_path_buf(), depth, ancestor);
                        self.stack.push(new_task);
                    }
                    entry_res
                }
                Err(err) => Err(err),
            };
            dir_entries.push(entry);
        }

        // Apply custom sorting if provided
        if let Some(ref mut sort_fn) = self.opts.sorter {
            dir_entries.sort_by(|a, b| match (a, b) {
                (&Ok(ref ea), &Ok(ref eb)) => sort_fn(ea, eb),
                (Err(_), Ok(_)) => std::cmp::Ordering::Less,
                (Ok(_), Err(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => std::cmp::Ordering::Equal,
            });
        }

        // Apply grouping (directories first) if requested
        if self.opts.group_dir {
            dir_entries.sort_by(|a, b| {
                let a_is_dir = a.as_ref().map(|e| e.is_dir()).unwrap_or(false);
                let b_is_dir = b.as_ref().map(|e| e.is_dir()).unwrap_or(false);
                b_is_dir.cmp(&a_is_dir)
            });
        }

        Ok(Some(DirNode::new(task.into_path(), depth, dir_entries)))
    }
}
