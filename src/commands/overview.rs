//! `skiman overview`: every skill on this machine as Markdown, for a person
//! who wants to see what each one is and where it came from. The canon's
//! skills come by group with their summary, kind, source, install
//! day, whether they are enabled and whether they have local modifications;
//! then each linked folder as one section, with each linked skill's summary
//! (ADR 0011) and whether it is distributed; then the skill skiman brings,
//! and the skills other managers keep, which are only listed. Read-only;
//! takes no lock. Spawns `git log` (read-only) for install days that
//! `skiman.toml` does not record.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;

use super::list::{canon_skills, distributed, source_text, yes_no};
use crate::Exit;
use crate::canon::Canon;
use crate::elsewhere::{self, OutsideSkill};
use crate::failure::{Failure, finish};
use crate::git;
use crate::linked::summary::About;
use crate::linked::{self, Linked, Shown};
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::project::root::written_form;
use crate::skiman_manual;
use crate::sync;

const CANON_HEADER: [&str; 7] = [
    "Skill",
    "Summary",
    "Kind",
    "Source",
    "Installed",
    "Enabled",
    "Modified",
];
const LINKED_HEADER: [&str; 3] = ["Skill", "Summary", "Distributed"];
const ELSEWHERE_HEADER: [&str; 4] = ["Skill", "Managed by", "Location", "Read by"];
/// Under a linked folder's table that shows a stand-in.
const STAND_IN_NOTE: &str = "A summary in italics stands in for a missing `metadata.summary`: \
     it is the first sentence of the skill's description.";
/// The summary of the skill skiman brings, which has no `[skills.<name>]` table.
const MANUAL_SUMMARY: &str = "How to change skills and global instructions with skiman";
/// What a day marked `(git)` means.
const GIT_DAY_NOTE: &str = "An installed day marked (git) is the day of the latest commit that added \
     the skill's SKILL.md to the canon, which can be later than the real install; \
     `installed = YYYY-MM-DD` in its [skills.<name>] table records the real day.";

pub fn run() -> Exit {
    finish(overview())
}

fn overview() -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = Canon::from_config(&paths)?;
    let manifest = Manifest::load(&repo)?;
    // Groups in name order; skills without one come last.
    let mut groups: BTreeMap<String, Vec<[String; 7]>> = BTreeMap::new();
    let mut ungrouped = Vec::new();
    let skills = canon_skills(&repo, &manifest)?;
    for skill in &skills {
        let (name, path) = (&skill.placed.name, skill.placed.path());
        let info = manifest.info(name);
        let source = match &skill.source {
            None => info.origin.clone(),
            Some(source) => Some(match &info.origin {
                Some(origin) => format!("{}; {origin}", source_text(source)),
                None => source_text(source),
            }),
        };
        let modified = if skill.conflicts {
            format!("{}, unresolved conflicts", skill.modified)
        } else {
            skill.modified.clone()
        };
        let installed = info
            .installed
            .clone()
            .or_else(|| install_commit_day(repo.root(), &path).map(|day| format!("{day} (git)")));
        let row = [
            name.clone(),
            or_dash(info.summary.clone()),
            skill.kind().to_owned(),
            or_dash(source),
            or_dash(installed),
            yes_no(skill.enabled),
            modified,
        ];
        match &skill.placed.group {
            Some(group) => groups.entry(group.clone()).or_default().push(row),
            None => ungrouped.push(row),
        }
    }
    let found = elsewhere::find(&paths, &repo);

    out!("# Skills\n");
    out!("## In the canon ({})\n", skills.len());
    for (group, rows) in &groups {
        out!("### {} ({})\n", cell(group), rows.len());
        table(&CANON_HEADER, rows);
    }
    if !ungrouped.is_empty() {
        out!("### No group ({})\n", ungrouped.len());
        table(&CANON_HEADER, &ungrouped);
    }
    let linked = Linked::read(paths.home(), &repo, &manifest);
    let kept = sync::kept_names(&paths, &repo, &manifest, &linked);
    for (folder, shown) in linked.shown(&repo.layout()?, &kept) {
        let path = folder.path.as_ref().expect("a shown folder has a path");
        let (id, path) = (&folder.id, written_form(paths.home(), path));
        let Shown::Skills(skills) = shown else {
            out!("## Linked folder {id}: `{path}`\n");
            out!("Not on this machine.\n");
            continue;
        };
        out!("## Linked folder {id} ({}): `{path}`\n", skills.len());
        if skills.is_empty() {
            continue;
        }
        let mut stand_in = false;
        let rows: Vec<[String; 3]> = skills
            .into_iter()
            .map(|(name, lost)| {
                let about = std::fs::read_to_string(folder.skill_dir(&name).join("SKILL.md"))
                    .ok()
                    .and_then(|text| About::read(&text))
                    .unwrap_or_default();
                let summary = match (about.summary, about.description) {
                    (Some(summary), _) => summary,
                    (None, Some(description)) if !linked::stand_in(&description).is_empty() => {
                        stand_in = true;
                        format!("*{}*", linked::stand_in(&description).replace('*', "\\*"))
                    }
                    _ => "-".to_owned(),
                };
                [name, summary, distributed(lost)]
            })
            .collect();
        table(&LINKED_HEADER, &rows);
        if stand_in {
            out!("{STAND_IN_NOTE}\n");
        }
    }
    out!("## Built into skiman (1)\n");
    table(
        &CANON_HEADER,
        &[[
            skiman_manual::NAME.to_owned(),
            MANUAL_SUMMARY.to_owned(),
            "built-in".to_owned(),
            format!("skiman {}", env!("CARGO_PKG_VERSION")),
            "-".to_owned(),
            "yes".to_owned(),
            "-".to_owned(),
        ]],
    );
    out!("## Not managed by skiman ({})\n", found.skills.len());
    out!("skiman lists these here and never changes them.\n");
    if !found.skills.is_empty() {
        let rows: Vec<[String; 4]> = found
            .skills
            .iter()
            .map(|skill| elsewhere_row(skill, paths.home()))
            .collect();
        table(&ELSEWHERE_HEADER, &rows);
    }
    out!("## Notes\n");
    for note in std::iter::once(GIT_DAY_NOTE.to_owned()).chain(found.notes) {
        out!("- {note}");
    }
    Ok(Exit::Success)
}

fn elsewhere_row(skill: &OutsideSkill, home: &Path) -> [String; 4] {
    [
        skill.name.clone(),
        skill.owner.clone(),
        written_form(home, &skill.location),
        skill.read_by.join(", "),
    ]
}

/// The day (`YYYY-MM-DD`) of the latest commit that added the `SKILL.md`
/// of the skill folder `skill`, or created it as a copy of another file (a
/// skill removed and installed again counts from its last install),
/// following renames (a move to another group is one); `None` when the
/// canon is not in a git repo or the file was never committed.
fn install_commit_day(root: &Path, skill: &str) -> Option<String> {
    let output = git::query_command(root)
        // `--follow` sees a skill adapted from another as a copy (C) of it:
        // that commit is the skill's add. No signature lines before the day.
        .args([
            "log",
            "--follow",
            "--diff-filter=AC",
            "--no-show-signature",
            "--format=%as",
            "--",
        ])
        .arg(format!("{skill}/SKILL.md"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let days = String::from_utf8(output.stdout).ok()?;
    let day = days.lines().next()?;
    let is_day = day.len() == 10
        && day.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    is_day.then(|| day.to_owned())
}

fn or_dash(value: Option<String>) -> String {
    value.unwrap_or_else(|| "-".to_owned())
}

/// A Markdown table with a blank line after it.
fn table<const N: usize>(header: &[&str; N], rows: &[[String; N]]) {
    let line = |cells: Vec<String>| format!("| {} |", cells.join(" | "));
    out!("{}", line(header.iter().map(|h| (*h).to_owned()).collect()));
    out!("{}", line(vec!["---".to_owned(); N]));
    for row in rows {
        out!("{}", line(row.iter().map(|c| cell(c)).collect()));
    }
    out!();
}

/// `text` safe inside a table cell: `|` escaped, line breaks as spaces.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::cell;

    #[test]
    fn a_cell_keeps_the_table_whole() {
        assert_eq!(cell("a | b\nc\r\nd"), "a \\| b c  d");
    }
}
