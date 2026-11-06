//! Rules on `projects/`, the mirrors of project items (plan "Rules"):
//! `project-path` on the layout, and the safety rules on every item. The
//! skill frontmatter and name rules do not apply: the mirror records what
//! the tools load, and they load a skill without `name` or `description`, in
//! a folder of any name.

use std::collections::BTreeSet;
use std::io;

use super::config::Records;
use super::secrets::{self, Found};
use super::skill::{self, CONFLICT_MARKERS};
use super::{LIMITS, Violation, readable, rule};
use crate::project::item::{MIRROR_DIR, read_mirror_of};
use crate::tree::{Kind, Tree, is_ignored};

/// Git's own conflict markers: what a conflicted merge leaves in a file.
const GIT_MARKERS: &[&str] = &["<<<<<<< ", ">>>>>>> "];

/// `project-path`: every entry under `projects/` is inside a configured
/// root id and part of a project item.
pub fn check_layout(
    tree: &dyn Tree,
    records: &Records,
    out: &mut BTreeSet<Violation>,
) -> io::Result<()> {
    let roots = records.projects();
    for entry in tree.children(MIRROR_DIR)? {
        if is_ignored(&entry.path) {
            continue;
        }
        let id = crate::tree::basename(&entry.path);
        let Some((_, skill_dirs)) = roots.iter().find(|(r, _)| r == id) else {
            out.insert(Violation::new(
                &entry.path,
                None,
                rule::PROJECT_PATH,
                format!(
                    "`{id}` is not a configured project root; add [projects.{id}] to skiman.toml again, or `git rm -r {}`",
                    entry.path
                ),
            ));
            continue;
        };
        if entry.kind != Kind::Dir {
            out.insert(Violation::new(
                &entry.path,
                None,
                rule::PROJECT_PATH,
                "must be a folder holding the mirror of a project root",
            ));
            continue;
        }
        for stray in read_mirror_of(tree, id, skill_dirs)?.strays {
            out.insert(Violation::new(
                &stray,
                None,
                rule::PROJECT_PATH,
                format!(
                    "belongs to no project item (an instruction file or a project skill); restore the configuration that made it one, or `git rm -r {stray}`"
                ),
            ));
        }
    }
    Ok(())
}

/// Every mirror item path of the configured roots in `tree`.
pub fn item_paths(tree: &dyn Tree, records: &Records) -> io::Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    for (id, skill_dirs) in records.projects() {
        for item in read_mirror_of(tree, &id, &skill_dirs)?.items {
            paths.insert(item.mirror_rel());
        }
    }
    Ok(paths)
}

/// The safety rules on the project item at `path` (`projects/<id>/<rel>`):
/// a real file or folder, sizes, `SKILL.md` and no symlinks in a skill,
/// conflict markers (skiman's and git's), secrets.
pub fn check_item(
    tree: &dyn Tree,
    path: &str,
    out: &mut BTreeSet<Violation>,
    found: &mut Vec<Found>,
) -> io::Result<()> {
    match tree.kind(path)? {
        None => {}
        Some(Kind::Dir) => {
            skill::check_folder(tree, path, out, found)?;
            for entry in tree.walk(path)? {
                if !is_ignored(&entry.path) && readable(entry.kind) {
                    check_git_markers(&entry.path, &tree.read(&entry.path)?, true, out);
                }
            }
        }
        Some(Kind::Symlink) => {
            out.insert(Violation::new(
                path,
                None,
                rule::SYMLINK,
                "a project item's mirror must be a real file or folder, not a symlink",
            ));
        }
        Some(Kind::Special) => {
            out.insert(Violation::new(
                path,
                None,
                rule::PROJECT_PATH,
                "a project item's mirror must be a regular file or a folder",
            ));
        }
        Some(kind @ Kind::File { size, .. }) => {
            if !readable(kind) {
                out.insert(Violation::new(
                    path,
                    None,
                    rule::FILE_SIZE,
                    format!(
                        "{size} bytes; a project instruction file may be at most {} bytes",
                        LIMITS.file_bytes
                    ),
                ));
                return Ok(());
            }
            let content = tree.read(path)?;
            check_git_markers(path, &content, false, out);
            secrets::scan(path, &content, found);
        }
    }
    Ok(())
}

/// Lines starting with `<<<<<<< ` or `>>>>>>> `. In a skill folder
/// (`in_skill`), lines that start skiman's own markers are left to the skill
/// rules, which report them already.
fn check_git_markers(path: &str, content: &[u8], in_skill: bool, out: &mut BTreeSet<Violation>) {
    for (index, line) in content.split(|b| *b == b'\n').enumerate() {
        let git = GIT_MARKERS.iter().any(|m| line.starts_with(m.as_bytes()));
        let skimans = CONFLICT_MARKERS
            .iter()
            .any(|m| line.starts_with(m.as_bytes()));
        if git && !(in_skill && skimans) {
            out.insert(Violation::new(
                path,
                Some(index + 1),
                rule::UNRESOLVED_CONFLICT,
                "conflict marker; resolve the conflict and remove the markers",
            ));
        }
    }
}
