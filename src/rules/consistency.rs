//! Consistency (spec Rules): every `source` table has a skill folder, a lock
//! entry and an upstream snapshot whose tree hash matches the lock; every lock
//! entry and every `upstream/<n>` folder has a `source` table. Own skills have
//! none of the three. Records whose skill folder is gone are leftover records
//! (exit 3). Nothing is guessed: each piece is reported on its own.

use std::collections::BTreeSet;
use std::io;

use super::config::{LOCKFILE, MANIFEST, Records};
use super::{Violation, rule};
use crate::tree::{Kind, Tree, tree_hash};

pub fn check(
    tree: &dyn Tree,
    records: &Records,
    names: &BTreeSet<String>,
    out: &mut BTreeSet<Violation>,
) -> io::Result<()> {
    if !records.parsed() {
        return Ok(());
    }
    let lock_names = records.lock_skills();
    for name in names {
        let upstream = format!("upstream/{name}");
        let has_folder = tree.kind(&format!("skills/{name}"))?.is_some();
        let has_source = records.source(name).is_some();
        let has_lock = lock_names.contains(name);
        let upstream_kind = tree.kind(&upstream)?;

        if !has_folder {
            let leftover = |path: &str, line: Option<usize>, what: String| {
                Violation::new(
                    path,
                    line,
                    rule::LEFTOVER_RECORD,
                    format!(
                        "{what}, but skills/{name} does not exist; run `skiman remove {name} --yes` (after a hand rename, rename this too)"
                    ),
                )
            };
            if has_source {
                out.insert(leftover(
                    MANIFEST,
                    records.source_line(name),
                    format!("leftover [skills.{name}.source] table"),
                ));
            }
            if has_lock {
                out.insert(leftover(
                    LOCKFILE,
                    records.lock_entry_line(name),
                    format!("leftover lock entry for {name}"),
                ));
            }
            if upstream_kind.is_some() {
                out.insert(leftover(
                    &upstream,
                    None,
                    "leftover upstream snapshot".to_owned(),
                ));
            }
            continue;
        }

        if has_source {
            if !has_lock {
                out.insert(Violation::new(
                    MANIFEST,
                    records.source_line(name),
                    rule::CONSISTENCY,
                    format!("[skills.{name}.source] has no entry in skiman.lock"),
                ));
            }
            if upstream_kind.is_none() {
                out.insert(Violation::new(
                    MANIFEST,
                    records.source_line(name),
                    rule::CONSISTENCY,
                    format!("[skills.{name}.source] has no upstream snapshot {upstream}"),
                ));
            }
            match upstream_kind {
                Some(Kind::Dir) => {
                    if let Some(locked) = records.locked_snapshot(name) {
                        let actual = tree_hash(tree, &upstream)?;
                        if actual != locked {
                            out.insert(Violation::new(&upstream, None, rule::SNAPSHOT_HASH, format!("tree hash {actual} does not match the snapshot {locked} in skiman.lock; upstream snapshots are never edited by hand, so restore it")));
                        }
                    }
                }
                Some(_) => {
                    out.insert(Violation::new(
                        &upstream,
                        None,
                        rule::CONSISTENCY,
                        "the upstream snapshot must be a folder",
                    ));
                }
                None => {}
            }
        } else {
            let own =
                format!("skills/{name} has no [skills.{name}.source] table, so it is an own skill");
            if has_lock {
                out.insert(Violation::new(LOCKFILE, records.lock_entry_line(name), rule::CONSISTENCY, format!("lock entry for {name}, but {own}; delete the entry, or restore the source table")));
            }
            if upstream_kind.is_some() {
                out.insert(Violation::new(&upstream, None, rule::CONSISTENCY, format!("upstream snapshot, but {own}; delete the snapshot, or restore the source table")));
            }
        }
    }
    Ok(())
}
