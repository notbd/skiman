//! Reading one linked folder (spec "Reading a linked folder"): a folder
//! whose top level holds a `SKILL.md` is one skill (listed, but its
//! entries are not read); any other folder is read one level deep. A
//! top-level `SKILL.md` that cannot be probed leaves the shape unknown: a
//! read error of the whole id, like a folder that cannot be listed. One
//! listing, one metadata call per entry, only links resolved and only
//! candidate folders probed for a `SKILL.md`; no content is read, so
//! `sync --hook` stays fast.

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::path::{self, Checked, Listings, Stop, resolve_in};
use crate::fsutil::absolute_lexical;
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
    /// to, as far as it resolves (a dangling one's target and one that
    /// cannot be entered included, a regular file not): these are
    /// protected like the registered path.
    pub targets: Vec<PathBuf>,
}

/// Why a whole folder cannot be read: its links and names stay as they are.
pub type ReadError = String;

/// Reads the folder at `dir`, the registered path's realpath.
/// `breaks` says why a path of a child link, or a link met on the way to
/// it, breaks the path rules; `listings` lists this folder (or gives the
/// listing it already holds) and resolves child links.
pub fn read(
    dir: &Path,
    breaks: &dyn Fn(&Path, Checked) -> Option<String>,
    listings: &mut Listings,
) -> Result<Contents, ReadError> {
    let cannot = |what: &Path, e: io::Error| format!("cannot read {}: {e}", what.display());
    let mut contents = Contents::default();
    let top = dir.join("SKILL.md");
    match std::fs::metadata(&top) {
        Ok(meta) if meta.is_file() => contents.single = true,
        Ok(_) => {}
        Err(e) if path::is_missing(&e) => {}
        Err(e) => return Err(cannot(&top, e)),
    }
    // Listed for a single skill too: a folder that cannot be listed is a
    // read error of the whole id whatever its shape.
    let listed = listings.list(dir).map_err(|e| cannot(dir, e))?.to_vec();
    if contents.single {
        return Ok(contents);
    }
    // The folder's own `SKILL.md`, probed above, is no entry: matched by
    // inode, so another spelling of it on a volume that ignores case is too.
    let top_ino = std::fs::symlink_metadata(&top).ok().map(|m| m.ino());
    let mut names: Vec<OsString> = listed
        .iter()
        .filter(|(_, ino)| Some(*ino) != top_ino)
        .map(|(name, _)| name.clone())
        .collect();
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
            let target = match std::fs::read_link(&entry) {
                Ok(target) => dir.join(target),
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(cannot(&entry, e)),
            };
            if !child_link(&entry, &target, breaks, listings, &mut contents) {
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
                format!("is a linked skill folder that is not distributed: {reason}"),
            ));
            continue;
        }
        contents.skills.push(name);
    }
    Ok(contents)
}

/// Checks the child link `entry`, whose target is `target` (joined to the
/// folder): records where it resolves when that is protected and its
/// problem when it has one. `true` when it may be a skill (it resolves to
/// a folder, or to something that cannot be entered).
fn child_link(
    entry: &Path,
    target: &Path,
    breaks: &dyn Fn(&Path, Checked) -> Option<String>,
    listings: &mut Listings,
    contents: &mut Contents,
) -> bool {
    let resolved = resolve_in(target, listings);
    if let Some(Stop::Loop) = resolved.stop {
        contents.problems.push((
            entry.to_owned(),
            "is a link that loops; it is not distributed".to_owned(),
        ));
        return false;
    }
    // A regular file is not a skill and not a folder: it protects nothing.
    if resolved.stop.is_none() && resolved.path.is_file() {
        return false;
    }
    // As for a registration: the target as written, normalised lexically,
    // every link met after the entry itself (which is in the linked
    // folder), and the realpath.
    let written = absolute_lexical(Path::new("/"), target);
    let broken = breaks(&written, Checked::Path)
        .or_else(|| {
            resolved
                .links
                .iter()
                .find_map(|link| breaks(link, Checked::Link))
        })
        .or_else(|| breaks(&resolved.path, Checked::Path));
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
