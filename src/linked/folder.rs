//! Reading one linked folder (spec "Reading a linked folder"): a folder
//! whose top level holds a `SKILL.md` is one skill; any other folder is read
//! one level deep. One listing, one metadata call per entry, only links
//! resolved and only candidate folders probed for a `SKILL.md`; no content
//! is read, so `sync --hook` stays fast.

use std::io;
use std::path::{Path, PathBuf};

use super::path::{self, Stop, resolve};
use crate::names;

/// What one linked folder holds.
#[derive(Debug, Clone, Default)]
pub struct Contents {
    /// The folder is itself one skill, named after its id.
    pub single: bool,
    /// The linked skills, in name order.
    pub skills: Vec<String>,
    /// Entries whose `SKILL.md` cannot be probed (permissions, I/O), and the
    /// error: each keeps its existing link and its name (exit 3).
    pub unprobed: Vec<(String, String)>,
    /// The folder's own content problems: an invalid skill name, a dangling
    /// or looping child link, a child link that breaks the path rules. Each
    /// is reported and leaves the exit status unchanged.
    pub problems: Vec<(PathBuf, String)>,
    /// What every child link that does not break the path rules resolves
    /// to, as far as it resolves (a dangling one's target included): these
    /// are protected like the registered path.
    pub targets: Vec<PathBuf>,
}

/// Why a whole folder cannot be read: its links and names stay as they are.
pub type ReadError = String;

/// Reads the folder at `dir`, the registered path's realpath.
/// `breaks` says why a child link's path breaks the path rules.
pub fn read(dir: &Path, breaks: &dyn Fn(&Path) -> Option<String>) -> Result<Contents, ReadError> {
    let cannot = |what: &Path, e: io::Error| format!("cannot read {}: {e}", what.display());
    let mut contents = Contents::default();
    let top = dir.join("SKILL.md");
    match std::fs::metadata(&top) {
        Ok(meta) if meta.is_file() => {
            contents.single = true;
            return Ok(contents);
        }
        Ok(_) => {}
        Err(e) if path::is_missing(&e) => {}
        Err(e) => return Err(cannot(&top, e)),
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| cannot(dir, e))? {
        names.push(entry.map_err(|e| cannot(dir, e))?.file_name());
    }
    names.sort();
    for name in names {
        if name.as_encoded_bytes().starts_with(b".") {
            continue;
        }
        let entry = dir.join(&name);
        let meta = match std::fs::symlink_metadata(&entry) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(cannot(&entry, e)),
        };
        if meta.is_symlink() {
            match std::fs::read_link(&entry) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(cannot(&entry, e)),
            }
            if !child_link(&entry, breaks, &mut contents) {
                continue;
            }
        } else if !meta.is_dir() {
            continue;
        }
        let name = name.to_string_lossy().into_owned();
        let skill_md = entry.join("SKILL.md");
        match std::fs::metadata(&skill_md) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => continue,
            Err(e) if path::is_missing(&e) => continue,
            Err(e) => {
                contents.unprobed.push((name, cannot(&skill_md, e)));
                continue;
            }
        }
        if let Some(reason) = not_eligible(&name) {
            contents.problems.push((
                entry,
                format!("linked skill folder is not distributed: {reason}"),
            ));
            continue;
        }
        contents.skills.push(name);
    }
    Ok(contents)
}

/// Checks the child link `entry`: records its target when it is protected
/// and its problem when it has one. `true` when it may be a skill (it
/// resolves to a folder, or to something that cannot be entered).
fn child_link(
    entry: &Path,
    breaks: &dyn Fn(&Path) -> Option<String>,
    contents: &mut Contents,
) -> bool {
    let resolved = resolve(entry);
    if let Some(Stop::Loop) = resolved.stop {
        contents.problems.push((
            entry.to_owned(),
            "is a link that loops; it is not distributed".to_owned(),
        ));
        return false;
    }
    // The entry itself is the first link met; it is in the linked folder.
    // None at all: the entry stopped being a link since it was listed.
    let broken = resolved
        .links
        .get(1..)
        .unwrap_or_default()
        .iter()
        .chain([&resolved.path])
        .find_map(|path| breaks(path));
    if let Some(why) = broken {
        contents.problems.push((
            entry.to_owned(),
            format!("is a link that breaks the path rules ({why}); it is not distributed"),
        ));
        return false;
    }
    contents.targets.push(resolved.path.clone());
    match resolved.stop {
        Some(Stop::Missing) => {
            contents.problems.push((
                entry.to_owned(),
                format!(
                    "is a dangling link to {}; it is not distributed",
                    resolved.path.display()
                ),
            ));
            false
        }
        // Probed like any candidate, so its `SKILL.md` is reported unprobed.
        Some(_) => true,
        None => resolved.path.is_dir(),
    }
}

/// Why a linked skill named `name` is not distributed, as for a skill
/// folder in the canon; `None` when it is.
pub fn not_eligible(name: &str) -> Option<&'static str> {
    if names::is_reserved(name) {
        Some("its name is reserved")
    } else if !names::is_valid(name) {
        Some("its name is not a valid skill name")
    } else {
        None
    }
}
