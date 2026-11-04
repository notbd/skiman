//! Reading `skiman.lock` (spec Data formats). Not to be confused with the
//! repo lock `.skiman/lock` (`repo_lock`).

use std::collections::BTreeSet;

use toml_edit::{DocumentMut, Item};

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
}
