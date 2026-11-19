//! Project items and the mirror tree (project-scope.md "Mirror layout"):
//! `<root>/<rel>` on disk pairs with `projects/<id>/<rel>` in the canon,
//! so the mirror tree is the only record of what is managed.

use std::path::PathBuf;

use super::pattern::{Kind, shape};
use super::root::Root;
use super::synced::Synced;
use crate::tree::{self, Tree};

/// The folder of the canon that holds every mirror.
pub const MIRROR_DIR: &str = "projects";

/// One project item: its root, its root-relative path, kind and anchor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Item {
    pub root: String,
    pub rel: String,
    pub kind: Kind,
    /// Root-relative path of the project folder the item belongs to.
    pub anchor: String,
}

impl Item {
    /// The item at `rel` when `rel` has an item shape.
    pub fn at(root: &Root, rel: &str, is_dir: bool) -> Option<Item> {
        shape(rel, is_dir).map(|(kind, anchor)| Item {
            root: root.id.clone(),
            rel: rel.to_owned(),
            kind,
            anchor,
        })
    }

    /// Repo-relative path of the mirror: `projects/<id>/<rel>`.
    pub fn mirror_rel(&self) -> String {
        format!("{MIRROR_DIR}/{}/{}", self.root, self.rel)
    }

    pub fn disk(&self, root: &Root) -> PathBuf {
        root.dir.join(&self.rel)
    }

    pub fn anchor_dir(&self, root: &Root) -> PathBuf {
        if self.anchor.is_empty() {
            root.dir.clone()
        } else {
            root.dir.join(&self.anchor)
        }
    }
}

/// What the mirror of one root holds.
#[derive(Debug, Default)]
pub struct Mirror {
    /// Every path with an item shape, whatever kind of entry it is.
    pub items: Vec<Item>,
    /// Repo-relative paths of files under `projects/<id>/` that belong to no
    /// item (`project-path`).
    pub strays: Vec<String>,
}

/// Reads the mirror of `root` from `tree` (the canon or a view of it).
/// Skill folders are not entered; ignored names ([`tree::IGNORED_NAMES`]) are
/// skipped.
pub fn read_mirror(tree: &dyn Tree, root: &Root) -> std::io::Result<Mirror> {
    read_mirror_of(tree, &root.id)
}

/// [`read_mirror`] by id, for the rules (which read the configuration from
/// the tree, not from a resolved root).
pub fn read_mirror_of(tree: &dyn Tree, id: &str) -> std::io::Result<Mirror> {
    let base = format!("{MIRROR_DIR}/{id}");
    let mut mirror = Mirror::default();
    let mut skills: Vec<String> = Vec::new();
    for entry in tree.walk(&base)? {
        let rel = &entry.path[base.len() + 1..];
        if skills.iter().any(|s| rel.starts_with(s.as_str())) || tree::is_ignored(rel) {
            continue;
        }
        let is_dir = entry.kind == tree::Kind::Dir;
        let found = shape(rel, is_dir).or_else(|| (!is_dir).then(|| shape(rel, true)).flatten());
        match found {
            Some((kind, anchor)) => {
                if kind == Kind::Skill {
                    skills.push(format!("{rel}/"));
                }
                mirror.items.push(Item {
                    root: id.to_owned(),
                    rel: rel.to_owned(),
                    kind,
                    anchor,
                });
            }
            None if is_dir => {}
            None => mirror.strays.push(entry.path.clone()),
        }
    }
    Ok(mirror)
}

/// Every item skiman manages under `root`: the mirror's items, plus the
/// items only the synced hashes still know (deleted in the mirror), whose
/// path still has the recorded kind's shape. Sorted by path.
pub fn managed(tree: &dyn Tree, root: &Root, synced: &Synced) -> std::io::Result<Vec<Item>> {
    let mut items = read_mirror(tree, root)?.items;
    for (rel, kind) in synced.items(root) {
        if !items.iter().any(|i| i.rel == rel)
            && let Some(item) = Item::at(root, &rel, kind == Kind::Skill)
            && item.kind == kind
        {
            items.push(item);
        }
    }
    items.sort();
    Ok(items)
}
