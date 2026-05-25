use std::{
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::paths::SanitizedPath;
use itertools::Itertools;
use serde::{Deserialize, Serialize};

/// A list of absolute paths, with an associated display order.
///
/// Two `PathList` values are considered equal if they contain the same paths,
/// regardless of the order in which those paths were originally provided.
///
/// The paths can be retrieved in the original order using `ordered_paths()`.
#[derive(Default, Debug, Clone)]
pub struct PathList {
    /// The paths, in lexicographic order.
    paths: Arc<[PathBuf]>,
    /// The order in which the paths were provided.
    ///
    /// See `ordered_paths()` for a way to get the paths in the original order.
    order: Arc<[usize]>,
}

impl PartialEq for PathList {
    fn eq(&self, other: &Self) -> bool {
        self.paths == other.paths
    }
}

impl Eq for PathList {}

impl Hash for PathList {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.paths.hash(state);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedPathList {
    pub paths: String,
    pub order: String,
}

impl PathList {
    pub fn new<P: AsRef<Path>>(paths: &[P]) -> Self {
        // De-duplicate while preserving the first-seen index so that
        // `ordered_paths()` keeps the user's original order. Duplicates
        // can sneak in via `WorktreeStore::paths` when a worktree is
        // momentarily double-registered (e.g. mid git-discovery), which
        // produced corrupt PathLists in agent thread / terminal thread
        // metadata storage and made those rows invisible in the sidebar
        // because the lookup key never matched.
        let mut seen: collections::HashSet<PathBuf> = collections::HashSet::default();
        let mut indexed_paths: Vec<(usize, PathBuf)> = Vec::with_capacity(paths.len());
        for (ix, path) in paths.iter().enumerate() {
            let sanitized: PathBuf = SanitizedPath::new(path).into();
            if seen.insert(sanitized.clone()) {
                indexed_paths.push((ix, sanitized));
            }
        }
        indexed_paths.sort_by(|(_, a), (_, b)| a.cmp(b));
        let order = indexed_paths.iter().map(|e| e.0).collect::<Vec<_>>().into();
        let paths = indexed_paths
            .into_iter()
            .map(|e| e.1)
            .collect::<Vec<_>>()
            .into();
        Self { order, paths }
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Returns a new `PathList` with the given path removed.
    pub fn without_path(&self, path_to_remove: &Path) -> PathList {
        let paths: Vec<PathBuf> = self
            .ordered_paths()
            .filter(|p| p.as_path() != path_to_remove)
            .cloned()
            .collect();
        PathList::new(&paths)
    }

    /// Get the paths in lexicographic order.
    pub fn paths(&self) -> &[PathBuf] {
        self.paths.as_ref()
    }

    /// Get the paths in the lexicographic order.
    pub fn paths_owned(&self) -> Arc<[PathBuf]> {
        self.paths.clone()
    }

    /// Get the order in which the paths were provided.
    pub fn order(&self) -> &[usize] {
        self.order.as_ref()
    }

    /// Get the paths in the original order.
    pub fn ordered_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.order
            .iter()
            .zip(self.paths.iter())
            .sorted_by_key(|(i, _)| **i)
            .map(|(_, path)| path)
    }

    pub fn is_lexicographically_ordered(&self) -> bool {
        self.order.iter().enumerate().all(|(i, &j)| i == j)
    }

    pub fn deserialize(serialized: &SerializedPathList) -> Self {
        let paths_vec: Vec<PathBuf> = if serialized.paths.is_empty() {
            Vec::new()
        } else {
            serialized.paths.split('\n').map(PathBuf::from).collect()
        };

        let order_vec: Vec<usize> = serialized
            .order
            .split(',')
            .filter_map(|s| s.parse().ok())
            .collect();

        // Round-trip through `new` so that legacy rows with duplicate
        // path entries — e.g. metadata persisted while `WorktreeStore`
        // was mid git-discovery and double-registered the same worktree
        // — self-heal on first read instead of staying corrupt forever.
        // The `order` is reconstructed from the serialized indices when
        // they're well-formed, otherwise from input order.
        let order_valid = order_vec.len() == paths_vec.len();
        if order_valid {
            let mut indexed: Vec<(usize, PathBuf)> = paths_vec
                .into_iter()
                .zip(order_vec.iter().copied())
                .map(|(path, ix)| (ix, path))
                .collect();
            // Replay the original first-seen order through `new` so
            // duplicates collapse the same way they would on a fresh
            // construction. This loses any sort-key ambiguity but
            // preserves user-visible insertion order.
            indexed.sort_by_key(|(ix, _)| *ix);
            let originals: Vec<PathBuf> = indexed.into_iter().map(|(_, p)| p).collect();
            return Self::new(&originals);
        }

        Self::new(&paths_vec)
    }

    pub fn serialize(&self) -> SerializedPathList {
        use std::fmt::Write as _;

        let mut paths = String::new();
        for path in self.paths.iter() {
            if !paths.is_empty() {
                paths.push('\n');
            }
            paths.push_str(&path.to_string_lossy());
        }

        let mut order = String::new();
        for ix in self.order.iter() {
            if !order.is_empty() {
                order.push(',');
            }
            write!(&mut order, "{}", *ix).unwrap();
        }
        SerializedPathList { paths, order }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_list() {
        let list1 = PathList::new(&["a/d", "a/c"]);
        let list2 = PathList::new(&["a/c", "a/d"]);

        assert_eq!(list1.paths(), list2.paths(), "paths differ");
        assert_eq!(list1.order(), &[1, 0], "list1 order incorrect");
        assert_eq!(list2.order(), &[0, 1], "list2 order incorrect");

        // Same paths in different order are equal (order is display-only).
        assert_eq!(
            list1, list2,
            "same paths with different order should be equal"
        );

        let list1_deserialized = PathList::deserialize(&list1.serialize());
        assert_eq!(list1_deserialized, list1, "list1 deserialization failed");

        let list2_deserialized = PathList::deserialize(&list2.serialize());
        assert_eq!(list2_deserialized, list2, "list2 deserialization failed");

        assert_eq!(
            list1.ordered_paths().collect_array().unwrap(),
            [&PathBuf::from("a/d"), &PathBuf::from("a/c")],
            "list1 ordered paths incorrect"
        );
        assert_eq!(
            list2.ordered_paths().collect_array().unwrap(),
            [&PathBuf::from("a/c"), &PathBuf::from("a/d")],
            "list2 ordered paths incorrect"
        );
    }

    #[test]
    fn test_path_list_ordering() {
        let list = PathList::new(&["b", "a", "c"]);
        assert_eq!(
            list.paths(),
            &[PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
        );
        assert_eq!(list.order(), &[1, 0, 2]);
        assert!(!list.is_lexicographically_ordered());

        let serialized = list.serialize();
        let deserialized = PathList::deserialize(&serialized);
        assert_eq!(deserialized, list);

        assert_eq!(
            deserialized.ordered_paths().collect_array().unwrap(),
            [
                &PathBuf::from("b"),
                &PathBuf::from("a"),
                &PathBuf::from("c")
            ]
        );

        let list = PathList::new(&["b", "c", "a"]);
        assert_eq!(
            list.paths(),
            &[PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
        );
        assert_eq!(list.order(), &[2, 0, 1]);
        assert!(!list.is_lexicographically_ordered());

        let serialized = list.serialize();
        let deserialized = PathList::deserialize(&serialized);
        assert_eq!(deserialized, list);

        assert_eq!(
            deserialized.ordered_paths().collect_array().unwrap(),
            [
                &PathBuf::from("b"),
                &PathBuf::from("c"),
                &PathBuf::from("a"),
            ]
        );
    }

    /// Duplicate input paths must be collapsed. `WorktreeStore::paths`
    /// can yield the same worktree twice during git-discovery races
    /// (once with its main resolved, once falling back to its own folder
    /// path). When that snapshot was persisted in agent-thread or
    /// terminal-thread metadata as `PathList::new(&[a, a])`, the resulting
    /// list compared unequal to a clean `PathList::new(&[a])` and the
    /// stored row went invisible in the sidebar even though
    /// `entries_for_path` would otherwise have found it.
    #[test]
    fn test_path_list_dedups_input() {
        let dup = PathList::new(&["a", "a"]);
        let single = PathList::new(&["a"]);
        assert_eq!(
            dup, single,
            "PathList::new must collapse duplicate input paths"
        );
        assert_eq!(dup.paths(), &[PathBuf::from("a")]);
        assert_eq!(dup.order(), &[0]);

        let dup_multi = PathList::new(&["b", "a", "b", "c", "a"]);
        let clean_multi = PathList::new(&["b", "a", "c"]);
        assert_eq!(
            dup_multi, clean_multi,
            "PathList::new must dedup while preserving first-seen order"
        );
        // First-seen order is preserved: b, a, c
        assert_eq!(
            dup_multi.ordered_paths().collect_array().unwrap(),
            [
                &PathBuf::from("b"),
                &PathBuf::from("a"),
                &PathBuf::from("c"),
            ]
        );
    }

    /// Existing DB rows can hold duplicate-bearing serialized PathLists
    /// from before the dedup landed in `new`. Deserialize must collapse
    /// the duplicates so those rows self-heal on first read.
    #[test]
    fn test_path_list_deserialize_dedups_legacy_rows() {
        // The shape we observed in `sidebar_terminal_threads.folder_paths`:
        // path "a" stored twice, with the second copy as the second
        // element. This is exactly what the worktree-discovery race
        // produces when the same worktree gets snapshotted twice.
        let corrupt = SerializedPathList {
            paths: "a\na".to_string(),
            order: "0,1".to_string(),
        };
        let healed = PathList::deserialize(&corrupt);
        assert_eq!(
            healed,
            PathList::new(&["a"]),
            "deserialized PathList must dedup duplicate input paths"
        );
        assert_eq!(healed.paths(), &[PathBuf::from("a")]);
    }
}
