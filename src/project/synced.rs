//! Machine state: the synced hash of every project item, in
//! `<state dir>/projects-synced.toml` (plan "Mirror layout and machine
//! state"). Per root id: the root's realpath, and per item its kind and the
//! content hash both sides had when skiman last saw them equal.
//!
//! ```toml
//! [work]
//! root = "/Users/me/Work"
//!
//! [work.items]
//! "AGENTS.md" = "file sha256:…"
//! "p/.claude/skills/x" = "skill sha256:…"
//! ```
//!
//! A sync cache, not a journal: losing it loses no content. Entries recorded
//! for another realpath of the root (its `path` changed) or for a root id
//! that is no longer configured count as absent and are dropped.

use std::collections::BTreeMap;
use std::path::PathBuf;

use toml_edit::{DocumentMut, Item, Table, value};

use super::pattern::Kind;
use super::root::Root;
use crate::fsutil;
use crate::paths::Paths;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RootRecord {
    dir: String,
    items: BTreeMap<String, (Kind, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Synced {
    roots: BTreeMap<String, RootRecord>,
    changed: bool,
}

impl Synced {
    fn file(paths: &Paths) -> PathBuf {
        paths.state_dir().join("projects-synced.toml")
    }

    /// The recorded hashes; a missing or unreadable file records nothing.
    pub fn load(paths: &Paths) -> Synced {
        let Ok(text) = std::fs::read_to_string(Synced::file(paths)) else {
            return Synced::default();
        };
        let Ok(doc) = text.parse::<DocumentMut>() else {
            return Synced::default();
        };
        let roots = doc
            .iter()
            .filter_map(|(id, item)| {
                let dir = item.get("root")?.as_str()?.to_owned();
                let items = item
                    .get("items")
                    .and_then(Item::as_table_like)
                    .map(|t| {
                        t.iter()
                            .filter_map(|(rel, v)| {
                                let (kind, hash) = v.as_str()?.split_once(' ')?;
                                Some((rel.to_owned(), (Kind::parse(kind)?, hash.to_owned())))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some((id.to_owned(), RootRecord { dir, items }))
            })
            .collect();
        Synced {
            roots,
            changed: false,
        }
    }

    /// Drops the records of root `id` only when it is not configured at all
    /// (a root that is configured but unusable on this machine keeps them).
    pub fn forget_unconfigured(&mut self, configured: &[String]) {
        let before = self.roots.len();
        self.roots.retain(|id, _| configured.contains(id));
        self.changed |= self.roots.len() != before;
    }

    /// The synced hash of `rel` under `root`.
    pub fn get(&self, root: &Root, rel: &str) -> Option<&(Kind, String)> {
        self.roots
            .get(&root.id)
            .filter(|r| r.dir == root.dir.display().to_string())?
            .items
            .get(rel)
    }

    /// Every recorded item of `root`.
    pub fn items(&self, root: &Root) -> Vec<(String, Kind)> {
        self.roots
            .get(&root.id)
            .filter(|r| r.dir == root.dir.display().to_string())
            .map(|r| {
                r.items
                    .iter()
                    .map(|(rel, (k, _))| (rel.clone(), *k))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn set(&mut self, root: &Root, rel: &str, kind: Kind, hash: &str) {
        let dir = root.dir.display().to_string();
        let record = self.roots.entry(root.id.clone()).or_default();
        if record.dir != dir {
            *record = RootRecord {
                dir,
                items: BTreeMap::new(),
            };
        }
        let new = (kind, hash.to_owned());
        if record.items.get(rel) != Some(&new) {
            record.items.insert(rel.to_owned(), new);
            self.changed = true;
        }
    }

    pub fn remove(&mut self, root: &Root, rel: &str) {
        if let Some(record) = self.roots.get_mut(&root.id)
            && record.items.remove(rel).is_some()
        {
            self.changed = true;
        }
    }

    /// Drops every record of root `id` at or under the root-relative `folder`
    /// (`""` for all), whatever realpath they were recorded for.
    pub fn remove_under(&mut self, id: &str, folder: &str) {
        if let Some(record) = self.roots.get_mut(id) {
            let before = record.items.len();
            record.items.retain(|rel, _| {
                !(folder.is_empty() || rel == folder || rel.starts_with(&format!("{folder}/")))
            });
            self.changed |= record.items.len() != before;
        }
    }

    /// Drops every record of root `id`.
    pub fn remove_root(&mut self, id: &str) {
        self.changed |= self.roots.remove(id).is_some();
    }

    /// Writes the file when something changed (temporary file, then rename).
    pub fn save(&mut self, paths: &Paths) -> std::io::Result<()> {
        if !self.changed {
            return Ok(());
        }
        let mut doc = DocumentMut::new();
        for (id, record) in &self.roots {
            let mut table = Table::new();
            table.insert("root", value(&record.dir));
            let mut items = Table::new();
            for (rel, (kind, hash)) in &record.items {
                items.insert(rel, value(format!("{} {hash}", kind.as_str())));
            }
            table.insert("items", Item::Table(items));
            doc.insert(id, Item::Table(table));
        }
        std::fs::create_dir_all(paths.state_dir())?;
        fsutil::write_atomic(&Synced::file(paths), doc.to_string().as_bytes())?;
        self.changed = false;
        Ok(())
    }
}
