//! Reading `skiman.toml` (spec Data formats). Only what sync and the name
//! checks need is read here; the rules on its content are `skiman check`'s.

use std::collections::BTreeMap;

use toml_edit::{DocumentMut, Item};

use crate::failure::Failure;
use crate::repo::SkillRepo;

#[derive(Debug, Clone)]
pub struct Manifest {
    doc: DocumentMut,
}

/// The `[skills.<name>]` table of one skill, as far as distribution needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillEntry {
    /// `enabled`, default true.
    pub enabled: bool,
}

impl Manifest {
    pub fn load(repo: &SkillRepo) -> Result<Manifest, Failure> {
        let file = repo.manifest_file();
        let text = std::fs::read_to_string(&file)
            .map_err(|e| Failure::operational(format!("cannot read {}: {e}", file.display())))?;
        Manifest::parse(&text).map_err(|e| Failure::refused(format!("{}: {e}", file.display())))
    }

    pub fn parse(text: &str) -> Result<Manifest, String> {
        let doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
        Ok(Manifest { doc })
    }

    /// `[tools].enabled`, in file order; empty when absent. Non-string items are skipped.
    pub fn enabled_tools(&self) -> Vec<String> {
        self.doc
            .get("tools")
            .and_then(|t| t.get("enabled"))
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every `[skills.<name>]` table.
    pub fn skills(&self) -> BTreeMap<String, SkillEntry> {
        let Some(skills) = self.doc.get("skills").and_then(Item::as_table_like) else {
            return BTreeMap::new();
        };
        skills
            .iter()
            .map(|(name, item)| {
                let entry = SkillEntry {
                    enabled: item.get("enabled").and_then(Item::as_bool).unwrap_or(true),
                };
                (name.to_owned(), entry)
            })
            .collect()
    }
}

/// The text `init` writes into a new `skiman.toml`.
pub fn new_manifest_text(enabled_tools: &[&str]) -> String {
    let ids: Vec<String> = enabled_tools.iter().map(|id| format!("\"{id}\"")).collect();
    format!("[tools]\nenabled = [{}]\n", ids.join(", "))
}
