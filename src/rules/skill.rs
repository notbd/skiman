//! Rules on `skills/<name>/`: the folder, its files, symlinks, sizes and
//! unresolved conflicts. The frontmatter rules are in `frontmatter`.

use std::collections::BTreeSet;
use std::io;

use super::{LIMITS, Violation, frontmatter, readable, rule};
use crate::names;
use crate::tree::{Kind, Tree, basename, is_ignored};

/// Marker lines of unresolved text conflicts, matched as line prefixes.
pub const CONFLICT_MARKERS: &[&str] = &["<<<<<<< local (skiman)", ">>>>>>> upstream (skiman)"];
/// Basename suffixes of conflict records.
pub const CONFLICT_RECORD_SUFFIXES: &[&str] = &[".skiman-upstream", ".skiman-upstream-deleted"];
/// Folder that holds conflict records whose own path was taken.
pub const CONFLICTS_DIR: &str = ".skiman-conflicts";

const MIB: u64 = 1024 * 1024;

/// Every rule on the skill folder `skills/<name>` as a whole.
pub fn check_skill(tree: &dyn Tree, name: &str, out: &mut BTreeSet<Violation>) -> io::Result<()> {
    let dir = format!("skills/{name}");
    match tree.kind(&dir)? {
        None => return Ok(()),
        Some(Kind::Dir) => {}
        Some(Kind::Symlink) => {
            out.insert(Violation::new(
                &dir,
                None,
                rule::SYMLINK,
                "the skill folder is a symlink; skills must hold real files",
            ));
            return Ok(());
        }
        Some(_) => {
            out.insert(Violation::new(
                &dir,
                None,
                rule::SKILL_FOLDER,
                "not a folder; everything under skills/ must be a skill folder",
            ));
            return Ok(());
        }
    }
    if names::is_reserved(name) {
        out.insert(Violation::new(&dir, None, rule::SKILL_FOLDER, format!("`{name}` is a reserved skill name (reserved: managing-skills, synced, and any name starting with `.`)")));
    } else if !names::is_valid(name) {
        out.insert(Violation::new(&dir, None, rule::SKILL_FOLDER, format!("`{name}` is not a valid skill name: use 1 to {} lowercase letters, digits and single hyphens between them", names::MAX_LEN)));
    }
    let skill_md = format!("{dir}/SKILL.md");
    match tree.kind(&skill_md)? {
        None => {
            out.insert(Violation::new(
                &dir,
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
    for entry in tree.walk(&dir)? {
        if is_ignored(&entry.path) {
            continue;
        }
        if let Kind::File { size, .. } = entry.kind {
            files += 1;
            bytes += size;
        }
        check_entry(tree, &entry.path, entry.kind, out)?;
    }
    if files > LIMITS.files {
        out.insert(Violation::new(
            &dir,
            None,
            rule::FILE_COUNT,
            format!("{files} files; a skill may hold at most {}", LIMITS.files),
        ));
    }
    if bytes > LIMITS.skill_bytes {
        out.insert(Violation::new(
            &dir,
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
/// including the conflict-record rules on the folders above it.
pub fn check_written_file(
    tree: &dyn Tree,
    path: &str,
    out: &mut BTreeSet<Violation>,
) -> io::Result<()> {
    let Some(rest) = path.strip_prefix("skills/") else {
        return Ok(());
    };
    let parts: Vec<&str> = rest.split('/').collect();
    for depth in 2..parts.len() {
        let ancestor = format!("skills/{}", parts[..depth].join("/"));
        check_conflict_name(&ancestor, true, out);
    }
    if let Some(kind) = tree.kind(path)? {
        check_entry(tree, path, kind, out)?;
    }
    Ok(())
}

/// Rules on one entry inside a skill folder.
fn check_entry(
    tree: &dyn Tree,
    path: &str,
    kind: Kind,
    out: &mut BTreeSet<Violation>,
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
            if is_top_skill_md(path) {
                frontmatter::check(path, &content, out);
            }
        }
        _ => {}
    }
    Ok(())
}

/// `skills/<name>/SKILL.md`, not a deeper `SKILL.md`.
fn is_top_skill_md(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    parts.len() == 3 && parts[0] == "skills" && parts[2] == "SKILL.md"
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
