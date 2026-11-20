//! What `skiman check` on the working tree warns about linked skills (spec
//! "Checks", user story 14): problems inside them, what cannot be read
//! now, the names they share and a `[skills.<name>]` table that names one.
//! Warnings only, never a violation: the canon cannot fix content it does
//! not hold.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::summary::About;
use super::{Holder, Linked};
use crate::layout::Layout;
use crate::manifest::Manifest;
use crate::rules::{Violation, frontmatter, rule, skill_body};
use crate::sync::ItemKind;

/// After a frontmatter that does not parse.
const UNPARSABLE: &str = "; Antigravity drops a skill whose frontmatter does not parse";

/// Every warning, in id and then name order (what cannot be read first),
/// then the collisions and the skill folders of the canon at `root` a kept
/// link withholds (`kept` from [`crate::sync::kept_names`]), then the
/// `[skills.<name>]` tables.
pub fn warnings(
    root: &Path,
    linked: &Linked,
    manifest: &Manifest,
    layout: &Layout,
    kept: &BTreeMap<String, Holder>,
) -> Vec<String> {
    let enabled = manifest.enabled_tools();
    let mut out = Vec::new();
    for folder in &linked.folders {
        // What cannot be read now and the problems among its entries, as
        // sync and `doctor` say them.
        out.extend(
            super::canon::folder_items(linked, folder)
                .into_iter()
                .filter_map(|item| match item.kind {
                    ItemKind::Linked {
                        what: "unreadable" | "unprobed" | "entry",
                        message,
                        ..
                    } => Some(message),
                    _ => None,
                }),
        );
        for name in Linked::skill_names(folder) {
            let path = folder.skill_dir(&name).join("SKILL.md");
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    out.push(format!("cannot read {}: {e}", path.display()));
                    continue;
                }
            };
            let shown = path.display().to_string();
            let about = About::read(&String::from_utf8_lossy(&bytes));
            out.extend(
                frontmatter_warnings(&name, &bytes, about.as_ref())
                    .into_iter()
                    .map(|v| {
                        Violation {
                            path: shown.clone(),
                            ..v
                        }
                        .to_string()
                    }),
            );
            if about.is_some_and(|about| about.summary.is_none()) {
                out.push(
                    Violation::new(
                        &shown,
                        None,
                        rule::SKILL_INFO,
                        format!(
                            "linked skill {name} has no `metadata.summary`; overview shows the first sentence of its description instead"
                        ),
                    )
                    .to_string(),
                );
            }
            if let Some(message) = skill_body::warning(&bytes, &enabled) {
                out.push(Violation::new(&shown, None, rule::SKILL_BODY_SIZE, message).to_string());
            }
        }
    }
    let (_, collisions) = linked.names(layout, kept, |_| true);
    out.extend(collisions.iter().map(|c| c.message()));
    out.extend(
        super::withheld(root, layout, manifest, kept)
            .iter()
            .map(|w| w.message()),
    );
    for name in manifest.skills().keys() {
        if layout.find(name).is_none()
            && let Some((folder, _)) = linked.skill(name)
        {
            out.push(format!(
                "[skills.{name}] in skiman.toml names linked skill {name} of linked folder `{}`; a linked skill has no table (its summary is `metadata.summary` in its own SKILL.md), so remove it",
                folder.id
            ));
        }
    }
    out
}

/// The frontmatter rules on the `SKILL.md` (`bytes`, read as `about`) of
/// the linked skill `name`, as for a skill folder of that name in the
/// canon; the path of each is still to be set.
fn frontmatter_warnings(name: &str, bytes: &[u8], about: Option<&About>) -> Vec<Violation> {
    let mut found = BTreeSet::new();
    frontmatter::check(&format!("{name}/SKILL.md"), bytes, &mut found);
    found
        .into_iter()
        .map(|v| match v.rule {
            rule::FRONTMATTER | rule::DUPLICATE_KEY => Violation {
                message: format!("{}{UNPARSABLE}", v.message),
                ..v
            },
            rule::NAME_FOLDER => Violation {
                message: format!(
                    "`name` is `{}` but the linked skill is named `{name}`; they should be equal",
                    about.and_then(|a| a.name.as_deref()).unwrap_or_default()
                ),
                ..v
            },
            _ => v,
        })
        .collect()
}
