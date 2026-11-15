//! `skiman overview`: every skill on this machine as Markdown, for a person
//! who wants to see what each one is and where it came from. The skill
//! repo's skills come by group with their summary, kind, source, install
//! day, whether they are enabled and whether they have local modifications;
//! then the skill skiman brings, and the skills other managers keep, which
//! are only listed. Read-only; takes no lock. Spawns `git log` (read-only)
//! for install days that `skiman.toml` does not record.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;

use super::list::{modified, source_text, yes_no};
use crate::Exit;
use crate::elsewhere::{self, OutsideSkill};
use crate::failure::{Failure, finish};
use crate::git;
use crate::managing_skills;
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::rules;
use crate::tree::{Kind, Tree, WorkingTree};

const REPO_HEADER: [&str; 7] = [
    "Skill",
    "Summary",
    "Kind",
    "Source",
    "Installed",
    "Enabled",
    "Modified",
];
const ELSEWHERE_HEADER: [&str; 4] = ["Skill", "Managed by", "Location", "Read by"];
/// The summary of the skill skiman brings, which has no `[skills.<name>]` table.
const MANAGING_SKILLS_SUMMARY: &str = "How to change skills and global instructions with skiman";
/// What a day marked `(git)` means.
const GIT_DAY_NOTE: &str = "An installed day marked (git) is the day of the first commit that added \
     the skill's SKILL.md to the skill repo, which can be later than the real install; \
     `installed = YYYY-MM-DD` in its [skills.<name>] table records the real day.";

pub fn run() -> Exit {
    finish(overview())
}

fn overview() -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let manifest = Manifest::load(&repo)?;
    let tree = WorkingTree::new(repo.root());
    let io = |e| Failure::operational(format!("cannot read {}: {e}", repo.root().display()));
    let enabled = manifest.skills();
    // Groups in name order; skills without one come last.
    let mut groups: BTreeMap<String, Vec<[String; 7]>> = BTreeMap::new();
    let mut ungrouped = Vec::new();
    let mut count = 0;
    for name in repo.skill_folder_names().map_err(io)? {
        if tree.kind(&format!("skills/{name}")).map_err(io)? != Some(Kind::Dir) {
            continue;
        }
        count += 1;
        let info = manifest.info(&name);
        let conflicts = !rules::unresolved_conflicts(&tree, &name)
            .map_err(io)?
            .is_empty();
        let (kind, source, modified) = match manifest.source(&name) {
            None => ("own", info.origin.clone(), "-".to_owned()),
            Some(source) => {
                let text = source_text(&source);
                let text = match &info.origin {
                    Some(origin) => format!("{text}; {origin}"),
                    None => text,
                };
                (
                    "third-party",
                    Some(text),
                    modified(&tree, &name).map_err(io)?,
                )
            }
        };
        let modified = if conflicts {
            format!("{modified}, unresolved conflicts")
        } else {
            modified
        };
        let installed = info
            .installed
            .clone()
            .or_else(|| first_commit_day(repo.root(), &name).map(|day| format!("{day} (git)")));
        let row = [
            name.clone(),
            or_dash(info.summary.clone()),
            kind.to_owned(),
            or_dash(source),
            or_dash(installed),
            yes_no(enabled.get(&name).is_none_or(|e| e.enabled)),
            modified,
        ];
        match info.group {
            Some(group) => groups.entry(group).or_default().push(row),
            None => ungrouped.push(row),
        }
    }
    let found = elsewhere::find(&paths, &repo, &manifest);

    out!("# Skills\n");
    out!("## In the skill repo ({count})\n");
    for (group, rows) in &groups {
        out!("### {} ({})\n", cell(group), rows.len());
        table(&REPO_HEADER, rows);
    }
    if !ungrouped.is_empty() {
        out!("### No group ({})\n", ungrouped.len());
        table(&REPO_HEADER, &ungrouped);
    }
    out!("## Built into skiman (1)\n");
    table(
        &REPO_HEADER,
        &[[
            managing_skills::NAME.to_owned(),
            MANAGING_SKILLS_SUMMARY.to_owned(),
            "built-in".to_owned(),
            format!("skiman {}", env!("CARGO_PKG_VERSION")),
            "-".to_owned(),
            "yes".to_owned(),
            "-".to_owned(),
        ]],
    );
    out!("## Managed elsewhere ({})\n", found.skills.len());
    out!("skiman only lists these; it never changes them.\n");
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
    let location = match skill.location.strip_prefix(home) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => skill.location.display().to_string(),
    };
    [
        skill.name.clone(),
        skill.owner.clone(),
        location,
        skill.read_by.join(", "),
    ]
}

/// The day (`YYYY-MM-DD`) of the first commit that added
/// `skills/<name>/SKILL.md`, following renames; `None` when the skill repo
/// is not in a git repo or the file was never committed.
fn first_commit_day(root: &Path, name: &str) -> Option<String> {
    let output = git::query_command(root)
        .args(["log", "--follow", "--diff-filter=A", "--format=%as", "--"])
        .arg(format!("skills/{name}/SKILL.md"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let days = String::from_utf8(output.stdout).ok()?;
    days.lines().last().map(str::to_owned)
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
