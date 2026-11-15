//! The reminder `skiman check` prints for a skill that lacks what lets a
//! person tell it apart: a `summary`, a `group`, and where it came from (the
//! `source` table of a third-party skill, `origin` of an own skill). It is a
//! warning, never a violation: no command fails on it, and only `skiman
//! check` on the working tree prints it.

use std::io;

use super::config::{INFO_KEYS, Records};
use super::{Violation, folder_names, rule};
use crate::tree::{Kind, Tree};

/// One warning per skill folder under `skills/` that lacks any of the keys.
pub fn gaps(tree: &dyn Tree) -> io::Result<Vec<Violation>> {
    let records = Records::read(tree)?;
    let mut out = Vec::new();
    // A `skiman.toml` that does not parse is a violation already; every
    // skill would look as if it had none of the keys.
    if !records.manifest_parsed() {
        return Ok(out);
    }
    for name in folder_names(tree, "skills")? {
        if tree.kind(&format!("skills/{name}"))? != Some(Kind::Dir) {
            continue;
        }
        // A value of the wrong type is a `toml-value` violation already.
        let has = |key| records.skill_has(&name, key);
        let [summary, group, origin] = INFO_KEYS;
        let mut missing: Vec<&str> = [summary, group]
            .into_iter()
            .filter(|key| !has(key))
            .collect();
        if records.source(&name).is_none() && !has(origin) {
            missing.push(origin);
        }
        if missing.is_empty() {
            continue;
        }
        let keys: Vec<String> = missing.iter().map(|key| format!("`{key}`")).collect();
        out.push(Violation::new(
            format!("skills/{name}"),
            None,
            rule::SKILL_INFO,
            format!("[skills.{name}] in skiman.toml has no {}", keys.join(", ")),
        ));
    }
    Ok(out)
}
