//! Reading `skiman.lock` (spec Data formats). Not to be confused with the
//! repo lock `.skiman/lock` (`repo_lock`).

use std::collections::BTreeSet;

use toml_edit::{DocumentMut, Item, Table, value};

use crate::failure::Failure;
use crate::repo::SkillRepo;

#[derive(Debug, Clone, Default)]
pub struct Lockfile {
    doc: DocumentMut,
}

impl Lockfile {
    /// Reads `skiman.lock`; a missing file has no entries.
    pub fn load(repo: &SkillRepo) -> Result<Lockfile, Failure> {
        let file = repo.lockfile();
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Lockfile::default()),
            Err(e) => {
                return Err(Failure::operational(format!(
                    "cannot read {}: {e}",
                    file.display()
                )));
            }
        };
        let doc = text
            .parse::<DocumentMut>()
            .map_err(|e| Failure::refused(format!("{}: {e}", file.display())))?;
        Ok(Lockfile { doc })
    }

    /// Names that have a `[skills.<name>]` entry.
    pub fn skill_names(&self) -> BTreeSet<String> {
        self.doc
            .get("skills")
            .and_then(Item::as_table_like)
            .map(|t| t.iter().map(|(name, _)| name.to_owned()).collect())
            .unwrap_or_default()
    }

    /// The `[skills.<name>]` entry; a missing or non-string `snapshot` reads as empty.
    pub fn entry(&self, name: &str) -> Option<LockEntry> {
        let entry = self.doc.get("skills")?.get(name)?.as_table_like()?;
        let field = |key| entry.get(key).and_then(Item::as_str).map(str::to_owned);
        Some(LockEntry {
            commit: field("commit"),
            git_ref: field("ref"),
            snapshot: field("snapshot").unwrap_or_default(),
        })
    }
}

/// One `[skills.<name>]` entry of `skiman.lock` (spec Data formats).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockEntry {
    /// Absent for local-folder sources.
    pub commit: Option<String>,
    /// `source.ref` at this add or update; absent when `source.ref` is absent.
    pub git_ref: Option<String>,
    /// `sha256:<tree hash of upstream/<name>>`.
    pub snapshot: String,
}

/// `text` (a `skiman.lock`, possibly empty) with the entry of `name` set and
/// `version = 1` added when missing. Other entries keep their bytes.
pub fn with_entry(text: &str, name: &str, entry: &LockEntry) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    if !doc.contains_key("version") {
        doc.insert("version", value(1));
    }
    let mut table = Table::new();
    if let Some(commit) = &entry.commit {
        table.insert("commit", value(commit));
    }
    if let Some(git_ref) = &entry.git_ref {
        table.insert("ref", value(git_ref));
    }
    table.insert("snapshot", value(&entry.snapshot));
    table.decor_mut().set_prefix("\n");
    let skills =
        crate::manifest::subtable(doc.as_table_mut(), "skills").ok_or("`skills` is not a table")?;
    skills.insert(name, Item::Table(table));
    Ok(doc.to_string())
}

/// `text` (a `skiman.lock`) with the entry of `old` renamed to `new`, its
/// values unchanged. Everything else keeps its bytes.
pub fn with_entry_renamed(text: &str, old: &str, new: &str) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    crate::manifest::rename_skill_table(&mut doc, old, new);
    Ok(doc.to_string())
}
