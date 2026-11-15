//! Rules on a skill folder (`skills/<name>/` or `skills/<group>/<name>/`):
//! where it sits, the folder, its files, symlinks, sizes and unresolved
//! conflicts. The frontmatter rules are in `frontmatter`.

use std::collections::BTreeSet;
use std::io;

use super::secrets::{self, Found};
use super::{LIMITS, Violation, frontmatter, readable, rule};
use crate::layout::{Layout, skill_path};
use crate::names;
use crate::tree::{Kind, Tree, basename, is_ignored};

/// Marker lines of unresolved text conflicts, matched as line prefixes.
pub const CONFLICT_MARKERS: &[&str] = &["<<<<<<< local (skiman)", ">>>>>>> upstream (skiman)"];
/// Basename suffixes of conflict records.
pub const CONFLICT_RECORD_SUFFIXES: &[&str] = &[".skiman-upstream", ".skiman-upstream-deleted"];
/// Folder that holds conflict records whose own path was taken.
pub const CONFLICTS_DIR: &str = ".skiman-conflicts";

const MIB: u64 = 1024 * 1024;

/// Every rule on the skill folder of `name` as a whole, wherever it sits
/// (`skills/<name>` or `skills/<group>/<name>`), and on where it sits: a
/// name is in one place only, in a group folder with a valid name.
pub fn check_skill(
    tree: &dyn Tree,
    layout: &Layout,
    name: &str,
    out: &mut BTreeSet<Violation>,
    found: &mut Vec<Found>,
) -> io::Result<()> {
    let placed = layout.all_of(name);
    if placed.len() > 1 {
        let paths: Vec<String> = placed.iter().map(|p| p.path()).collect();
        for path in &paths {
            out.insert(Violation::new(
                path,
                None,
                rule::SKILL_FOLDER,
                format!(
                    "the skill name `{name}` is in {}; a name may be in one place only, whatever its group",
                    paths.join(" and ")
                ),
            ));
        }
    }
    for placed in placed {
        if let Some(group) = &placed.group {
            check_group(layout, group, out);
        }
        check_one(tree, &placed.path(), name, out, found)?;
    }
    Ok(())
}

/// The rules on a group folder's name.
fn check_group(layout: &Layout, group: &str, out: &mut BTreeSet<Violation>) {
    let dir = skill_path(None, group);
    if !names::is_valid(group) {
        out.insert(Violation::new(&dir, None, rule::SKILL_FOLDER, format!("`{group}` is not a valid group name: use 1 to {} lowercase letters, digits and single hyphens between them", names::MAX_LEN)));
    } else if let Some(skill) = layout.find(group) {
        out.insert(Violation::new(
            &dir,
            None,
            rule::SKILL_FOLDER,
            format!(
                "`{group}` names both this group folder and the skill {}; rename one of them",
                skill.path()
            ),
        ));
    }
}

fn check_one(
    tree: &dyn Tree,
    dir: &str,
    name: &str,
    out: &mut BTreeSet<Violation>,
    found: &mut Vec<Found>,
) -> io::Result<()> {
    match tree.kind(dir)? {
        None => return Ok(()),
        Some(Kind::Dir) => {}
        Some(Kind::Symlink) => {
            out.insert(Violation::new(
                dir,
                None,
                rule::SYMLINK,
                "the skill folder is a symlink; skills must hold real files",
            ));
            return Ok(());
        }
        Some(_) => {
            out.insert(Violation::new(
                dir,
                None,
                rule::SKILL_FOLDER,
                "not a folder; everything under skills/ must be a skill folder or a group folder of skill folders",
            ));
            return Ok(());
        }
    }
    if names::is_reserved(name) {
        out.insert(Violation::new(dir, None, rule::SKILL_FOLDER, format!("`{name}` is a reserved skill name (reserved: managing-skills, synced, and any name starting with `.`)")));
    } else if !names::is_valid(name) {
        out.insert(Violation::new(dir, None, rule::SKILL_FOLDER, format!("`{name}` is not a valid skill name: use 1 to {} lowercase letters, digits and single hyphens between them", names::MAX_LEN)));
    }
    check_content(tree, dir, true, out, found)
}

/// The rules on a skill folder's content, wherever it sits (`skills/<name>`
/// or a project skill under `projects/`): `SKILL.md` at the top, every
/// entry, the file count and the total size.
pub fn check_folder(
    tree: &dyn Tree,
    dir: &str,
    out: &mut BTreeSet<Violation>,
    found: &mut Vec<Found>,
) -> io::Result<()> {
    check_content(tree, dir, false, out, found)
}

/// [`check_folder`], with the frontmatter rules on its `SKILL.md` when
/// `frontmatter` is set (a skill in the skill repo).
fn check_content(
    tree: &dyn Tree,
    dir: &str,
    frontmatter: bool,
    out: &mut BTreeSet<Violation>,
    found: &mut Vec<Found>,
) -> io::Result<()> {
    let skill_md = format!("{dir}/SKILL.md");
    match tree.kind(&skill_md)? {
        None => {
            out.insert(Violation::new(
                dir,
                None,
                rule::SKILL_MD,
                "no SKILL.md at the top of the skill folder",
            ));
        }
        Some(Kind::Dir) | Some(Kind::Special) => {
            out.insert(Violation::new(
                &skill_md,
                None,
                rule::SKILL_MD,
                "SKILL.md must be a regular file",
            ));
        }
        // A symlinked SKILL.md is reported by the symlink rule below.
        Some(_) => {}
    }

    let (mut files, mut bytes) = (0usize, 0u64);
    for entry in tree.walk(dir)? {
        if is_ignored(&entry.path) {
            continue;
        }
        if let Kind::File { size, .. } = entry.kind {
            files += 1;
            bytes += size;
        }
        let top = frontmatter && entry.path == skill_md;
        check_entry(tree, &entry.path, entry.kind, top, out, found)?;
    }
    if files > LIMITS.files {
        out.insert(Violation::new(
            dir,
            None,
            rule::FILE_COUNT,
            format!("{files} files; a skill may hold at most {}", LIMITS.files),
        ));
    }
    if bytes > LIMITS.skill_bytes {
        out.insert(Violation::new(
            dir,
            None,
            rule::SKILL_SIZE,
            format!(
                "{bytes} bytes in total; a skill may hold at most {} MiB",
                LIMITS.skill_bytes / MIB
            ),
        ));
    }
    Ok(())
}

/// The per-file rules on one file a write command wrote under `skills/`,
/// including the conflict-record rules on the folders above it inside its
/// skill folder.
pub fn check_written_file(
    tree: &dyn Tree,
    layout: &Layout,
    path: &str,
    out: &mut BTreeSet<Violation>,
    found: &mut Vec<Found>,
) -> io::Result<()> {
    let Some(skill) = layout
        .skills()
        .iter()
        .map(|placed| placed.path())
        .find(|skill| {
            path.strip_prefix(skill.as_str())
                .is_some_and(|r| r.starts_with('/'))
        })
    else {
        return Ok(());
    };
    let inside: Vec<&str> = path[skill.len() + 1..].split('/').collect();
    for depth in 1..inside.len() {
        let ancestor = format!("{skill}/{}", inside[..depth].join("/"));
        check_conflict_name(&ancestor, true, out);
    }
    if let Some(kind) = tree.kind(path)? {
        let top = path == format!("{skill}/SKILL.md");
        check_entry(tree, path, kind, top, out, found)?;
    }
    Ok(())
}

/// Rules on one entry inside a skill folder.
/// `top_skill_md`: the entry is the `SKILL.md` of a skill in the skill
/// repo, which the frontmatter rules check.
fn check_entry(
    tree: &dyn Tree,
    path: &str,
    kind: Kind,
    top_skill_md: bool,
    out: &mut BTreeSet<Violation>,
    found: &mut Vec<Found>,
) -> io::Result<()> {
    if is_ignored(path) {
        return Ok(());
    }
    check_conflict_name(path, kind == Kind::Dir, out);
    match kind {
        Kind::Symlink => {
            out.insert(Violation::new(
                path,
                None,
                rule::SYMLINK,
                "symlinks are not allowed inside skills",
            ));
        }
        Kind::File { size, .. } if size > LIMITS.file_bytes => {
            out.insert(Violation::new(
                path,
                None,
                rule::FILE_SIZE,
                format!(
                    "{size} bytes; a file in a skill may be at most {} MiB",
                    LIMITS.file_bytes / MIB
                ),
            ));
        }
        kind if readable(kind) => {
            let content = tree.read(path)?;
            check_markers(path, &content, out);
            secrets::scan(path, &content, found);
            if top_skill_md {
                frontmatter::check(path, &content, out);
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_conflict_name(path: &str, is_dir: bool, out: &mut BTreeSet<Violation>) {
    let name = basename(path);
    if is_dir && name == CONFLICTS_DIR {
        out.insert(Violation::new(
            path,
            None,
            rule::UNRESOLVED_CONFLICT,
            "conflict records from an update; resolve them, then delete this folder",
        ));
    } else if CONFLICT_RECORD_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        out.insert(Violation::new(
            path,
            None,
            rule::UNRESOLVED_CONFLICT,
            "conflict record from an update; resolve the conflict, then delete it",
        ));
    }
}

fn check_markers(path: &str, content: &[u8], out: &mut BTreeSet<Violation>) {
    for (index, line) in content.split(|b| *b == b'\n').enumerate() {
        if CONFLICT_MARKERS
            .iter()
            .any(|m| line.starts_with(m.as_bytes()))
        {
            out.insert(Violation::new(
                path,
                Some(index + 1),
                rule::UNRESOLVED_CONFLICT,
                "conflict marker from an update; resolve the conflict and remove the markers",
            ));
        }
    }
}
