//! The reminder `skiman check` prints for a skill that lacks what lets a
//! person tell it apart: a `summary`, a group folder to sit in, and where it
//! came from (the `source` table of a third-party skill, `origin` of an own
//! skill). It is a warning, never a violation: no command fails on it, and
//! only `skiman check` on the working tree prints it.

use std::io;

use super::config::{INFO_KEYS, Records};
use super::{Violation, rule};
use crate::layout::Layout;
use crate::tree::{Kind, Tree};

/// One warning per skill folder under `skills/` that lacks any of them,
/// from `records` and `layout` as read from `tree`.
pub fn gaps(tree: &dyn Tree, records: &Records, layout: &Layout) -> io::Result<Vec<Violation>> {
    let mut out = Vec::new();
    // A `skiman.toml` that does not parse is a violation already; every
    // skill would look as if it had none of the keys.
    if !records.manifest_parsed() {
        return Ok(out);
    }
    for placed in layout.skills() {
        let (name, path) = (&placed.name, placed.path());
        if tree.kind(&path)? != Some(Kind::Dir) {
            continue;
        }
        // A value of the wrong type is a `toml-value` violation already.
        let has = |key| records.skill_has(name, key);
        let [summary, origin] = INFO_KEYS;
        let mut missing: Vec<&str> = [summary].into_iter().filter(|key| !has(key)).collect();
        if records.source(name).is_none() && !has(origin) {
            missing.push(origin);
        }
        let mut gaps = Vec::new();
        if !missing.is_empty() {
            let keys: Vec<String> = missing.iter().map(|key| format!("`{key}`")).collect();
            gaps.push(format!(
                "[skills.{name}] in skiman.toml has no {}",
                keys.join(", ")
            ));
        }
        if placed.group.is_none() {
            gaps.push(format!(
                "it is in no group folder; move it into one with `skiman mv {name} --group <group>`"
            ));
        }
        if !gaps.is_empty() {
            out.push(Violation::new(
                path,
                None,
                rule::SKILL_INFO,
                gaps.join("; "),
            ));
        }
    }
    Ok(out)
}
