use crate::format::IndexEntry;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeStatus {
    Committed, // In existing archive (#000000)
    Staged,    // Newly added ([+] glyph, #000080)
    Deleted,   // Marked for deletion
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlatTreeRow {
    pub id: String,
    pub name: String,
    pub path: String,
    pub node_type: NodeType,
    pub status: NodeStatus,
    pub size: u64,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// Box-drawing guide prefix, e.g. "│  ├─ "
    pub prefix: String,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceTree {
    /// Expanded directory paths set
    pub expanded_dirs: HashSet<String>,
    /// Selected node id
    pub selected_id: Option<String>,
    /// Committed entries (from inspected archive)
    pub committed_entries: Vec<IndexEntry>,
    /// Staged new items (disk_path, archive_path, size, is_dir)
    pub staged_items: Vec<(PathBuf, String, u64, bool)>,
    /// Set of deleted paths (from committed)
    pub deleted_paths: HashSet<String>,
}

impl WorkspaceTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.expanded_dirs.clear();
        self.selected_id = None;
        self.committed_entries.clear();
        self.staged_items.clear();
        self.deleted_paths.clear();
    }

    pub fn set_committed(&mut self, entries: Vec<IndexEntry>) {
        self.committed_entries = entries;
        self.staged_items.clear();
        self.deleted_paths.clear();
        // By default, expand root-level directories
        for entry in &self.committed_entries {
            if entry.is_dir && !entry.path.contains('/') {
                self.expanded_dirs.insert(entry.path.clone());
            }
        }
    }

    pub fn add_staged(&mut self, disk_path: PathBuf, archive_path: String, size: u64, is_dir: bool) {
        // Remove from deleted if re-added
        self.deleted_paths.remove(&archive_path);
        // Avoid duplicates
        if !self.staged_items.iter().any(|(_, p, _, _)| p == &archive_path) {
            self.staged_items.push((disk_path, archive_path.clone(), size, is_dir));
        }
        if is_dir {
            self.expanded_dirs.insert(archive_path);
        }
    }

    pub fn mark_deleted(&mut self, path: &str) {
        // If it was in staged_items, remove it
        self.staged_items.retain(|(_, p, _, _)| p != path && !p.starts_with(&format!("{}/", path)));
        // If it was committed, add to deleted_paths
        self.deleted_paths.insert(path.to_string());
        for entry in &self.committed_entries {
            if entry.path == path || entry.path.starts_with(&format!("{}/", path)) {
                self.deleted_paths.insert(entry.path.clone());
            }
        }
    }

    pub fn toggle_expand(&mut self, dir_path: &str) {
        if self.expanded_dirs.contains(dir_path) {
            self.expanded_dirs.remove(dir_path);
        } else {
            self.expanded_dirs.insert(dir_path.to_string());
        }
    }

    pub fn is_dirty(&self) -> bool {
        !self.staged_items.is_empty() || !self.deleted_paths.is_empty()
    }

    pub fn count_nodes(&self) -> (usize, usize) {
        let mut files = 0;
        let mut dirs = 0;

        for entry in &self.committed_entries {
            if self.deleted_paths.contains(&entry.path) {
                continue;
            }
            if entry.is_dir {
                dirs += 1;
            } else {
                files += 1;
            }
        }

        for (_, _, _, is_dir) in &self.staged_items {
            if *is_dir {
                dirs += 1;
            } else {
                files += 1;
            }
        }

        (files, dirs)
    }

    pub fn total_uncompressed_bytes(&self) -> (u64, i64) {
        // Returns (committed_active_bytes, staged_delta_bytes)
        let mut committed_active = 0u64;
        let mut deleted_bytes = 0u64;

        for entry in &self.committed_entries {
            if self.deleted_paths.contains(&entry.path) {
                deleted_bytes += entry.uncompressed_size;
            } else {
                committed_active += entry.uncompressed_size;
            }
        }

        let mut staged_bytes = 0u64;
        for (_, _, size, is_dir) in &self.staged_items {
            if !*is_dir {
                staged_bytes += *size;
            }
        }

        let delta = staged_bytes as i64 - deleted_bytes as i64;
        (committed_active, delta)
    }

    /// Flattens the hierarchical workspace into visible rows for virtualized rendering.
    pub fn flatten_visible_rows(&self) -> Vec<FlatTreeRow> {
        // Collect all active entries into a sorted trie/map
        #[derive(Debug)]
        struct TempNode {
            name: String,
            path: String,
            size: u64,
            is_dir: bool,
            status: NodeStatus,
            children: BTreeMap<String, TempNode>,
        }

        let mut root_children: BTreeMap<String, TempNode> = BTreeMap::new();

        // 1. Insert committed entries
        for entry in &self.committed_entries {
            let is_deleted = self.deleted_paths.contains(&entry.path);
            let status = if is_deleted {
                NodeStatus::Deleted
            } else {
                NodeStatus::Committed
            };

            let parts: Vec<&str> = entry.path.split('/').filter(|s| !s.is_empty()).collect();
            if parts.is_empty() {
                continue;
            }

            let mut current = &mut root_children;
            let mut current_path = String::new();

            for (idx, &part) in parts.iter().enumerate() {
                if !current_path.is_empty() {
                    current_path.push('/');
                }
                current_path.push_str(part);

                let is_last = idx == parts.len() - 1;
                let is_dir = if is_last { entry.is_dir } else { true };
                let size = if is_last { entry.uncompressed_size } else { 0 };

                let node = current.entry(part.to_string()).or_insert_with(|| TempNode {
                    name: part.to_string(),
                    path: current_path.clone(),
                    size,
                    is_dir,
                    status,
                    children: BTreeMap::new(),
                });

                if is_last {
                    node.size = entry.uncompressed_size;
                    node.is_dir = entry.is_dir;
                    node.status = status;
                }

                current = &mut node.children;
            }
        }

        // 2. Insert staged entries
        for (_, archive_path, size, is_dir) in &self.staged_items {
            let parts: Vec<&str> = archive_path.split('/').filter(|s| !s.is_empty()).collect();
            if parts.is_empty() {
                continue;
            }

            let mut current = &mut root_children;
            let mut current_path = String::new();

            for (idx, &part) in parts.iter().enumerate() {
                if !current_path.is_empty() {
                    current_path.push('/');
                }
                current_path.push_str(part);

                let is_last = idx == parts.len() - 1;
                let node_dir = if is_last { *is_dir } else { true };
                let node_size = if is_last { *size } else { 0 };

                let node = current.entry(part.to_string()).or_insert_with(|| TempNode {
                    name: part.to_string(),
                    path: current_path.clone(),
                    size: node_size,
                    is_dir: node_dir,
                    status: NodeStatus::Staged,
                    children: BTreeMap::new(),
                });

                if is_last {
                    node.size = *size;
                    node.is_dir = *is_dir;
                    node.status = NodeStatus::Staged;
                }

                current = &mut node.children;
            }
        }

        // Flatten recursively based on expanded_dirs
        let mut rows = Vec::new();

        fn walk(
            nodes: &BTreeMap<String, TempNode>,
            depth: usize,
            prefix: &str,
            expanded_dirs: &HashSet<String>,
            out: &mut Vec<FlatTreeRow>,
        ) {
            // Directories first, then files; deleted nodes hidden
            let mut visible: Vec<&TempNode> = nodes
                .values()
                .filter(|n| n.status != NodeStatus::Deleted)
                .collect();
            visible.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
            let count = visible.len();

            for (i, node) in visible.into_iter().enumerate() {
                let is_last = i + 1 == count;
                let is_expanded = expanded_dirs.contains(&node.path);
                let node_type = if node.is_dir {
                    NodeType::Directory
                } else {
                    NodeType::File
                };
                let branch = if is_last { "└─ " } else { "├─ " };

                out.push(FlatTreeRow {
                    id: node.path.clone(),
                    name: node.name.clone(),
                    path: node.path.clone(),
                    node_type,
                    status: node.status,
                    size: node.size,
                    depth,
                    is_dir: node.is_dir,
                    expanded: is_expanded,
                    prefix: format!("{}{}", prefix, branch),
                });

                if node.is_dir && is_expanded {
                    let child_prefix = format!("{}{}", prefix, if is_last { "   " } else { "│  " });
                    walk(&node.children, depth + 1, &child_prefix, expanded_dirs, out);
                }
            }
        }

        walk(&root_children, 0, "", &self.expanded_dirs, &mut rows);
        rows
    }
}
