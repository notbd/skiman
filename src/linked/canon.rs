//! `<canon>/linked` (spec "Sync"): one link per linked folder that was read,
//! pointing at its registered path with `~` expanded. Every link in it is
//! skiman's, under any name; nothing else in it is.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{Folder, Holder, Linked, State};
use crate::fsutil::{self, absolute_lexical, temporary_of};
use crate::sync::{Change, Item, ItemKind, Report};
use crate::test_hook;
use crate::tree::is_ignored;

/// Makes `<canon>/linked` hold the link `<id>` for each read folder, leaves
/// an unreadable folder's link exactly as it is, and removes every other
/// link (an id no longer registered, missing or breaking a rule, or a
/// temporary name a killed sync left). Returns the ids whose link is
/// confirmed to point at the registered path: only their skills are offered
/// to the tool directories.
pub fn reconcile(linked: &Linked, report: &mut Report) -> BTreeSet<String> {
    let dir = linked.dir();
    let mut confirmed = BTreeSet::new();
    let wanted: Vec<(&str, &PathBuf)> = linked
        .folders
        .iter()
        .filter(|f| matches!(f.state, State::Read(_)))
        .map(|f| {
            (
                f.id.as_str(),
                f.path.as_ref().expect("a read folder has a path"),
            )
        })
        .collect();
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if wanted.is_empty() {
                return confirmed;
            }
            if let Err(e) = std::fs::create_dir(dir) {
                report
                    .errors
                    .push(format!("cannot create {}: {e}", dir.display()));
                return confirmed;
            }
        }
        Err(e) => {
            report
                .errors
                .push(format!("cannot inspect {}: {e}", dir.display()));
            return confirmed;
        }
        Ok(meta) if !meta.is_dir() => {
            report
                .items
                .push(note(dir, dir.to_owned(), "blocked", blocked(dir, &meta)));
            return confirmed;
        }
        Ok(_) => {}
    }
    let entries = match entries(dir) {
        Ok(entries) => entries,
        Err(e) => {
            report
                .errors
                .push(format!("cannot read {}: {e}", dir.display()));
            return confirmed;
        }
    };
    let mut seen = BTreeSet::new();
    for (name, is_link) in entries {
        let path = dir.join(&name);
        let text = name.to_string_lossy();
        if is_ignored(&text) {
            continue;
        }
        let folder = linked.folders.iter().find(|f| f.id == text);
        let want = wanted.iter().find(|(id, _)| *id == text).map(|(_, p)| *p);
        if want.is_some() {
            seen.insert(text.clone().into_owned());
        }
        if !is_link {
            let message = match want {
                Some(_) => occupied(&path, &text),
                None => format!(
                    "{} is not a link skiman made, so skiman leaves it alone; move it out of {}",
                    path.display(),
                    dir.display()
                ),
            };
            report.items.push(note(dir, path, "occupied", message));
            continue;
        }
        let written = match std::fs::read_link(&path) {
            Ok(written) => written,
            Err(e) => {
                report
                    .errors
                    .push(format!("cannot read {}: {e}", path.display()));
                continue;
            }
        };
        let current = absolute_lexical(dir, &written);
        match (want, folder.map(|f| &f.state)) {
            (Some(want), _) if temporary_of(&name).is_none() => {
                // As written, not folded lexically: `a/link/../b` is not `a/b`
                // when `link` leads elsewhere.
                if written == *want {
                    confirmed.insert(text.into_owned());
                    continue;
                }
                match fsutil::relink(&path, dir, &current, Some(want)) {
                    Ok(true) => {
                        report.changes.push((Change::Retargeted, path));
                        confirmed.insert(text.into_owned());
                    }
                    Ok(false) => {}
                    Err(e) => report
                        .errors
                        .push(format!("cannot retarget {}: {e}", path.display())),
                }
            }
            (None, Some(State::Unreadable(_))) => {}
            _ => match fsutil::relink(&path, dir, &current, None) {
                Ok(true) => report.changes.push((Change::Removed, path)),
                Ok(false) => {}
                Err(e) => report
                    .errors
                    .push(format!("cannot remove {}: {e}", path.display())),
            },
        }
    }
    for (id, want) in wanted {
        if seen.contains(id) {
            continue;
        }
        let path = dir.join(id);
        test_hook::at("sync", &format!("link:{}", path.display()));
        // `symlink` never replaces: an entry that appeared since the
        // listing occupies the id like one the listing found.
        match std::os::unix::fs::symlink(want, &path) {
            Ok(()) => {
                report.changes.push((Change::Linked, path));
                confirmed.insert(id.to_owned());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let message = occupied(&path, id);
                report.items.push(note(dir, path, "occupied", message));
            }
            Err(e) => report
                .errors
                .push(format!("cannot link {}: {e}", path.display())),
        }
    }
    confirmed
}

/// The problems `<canon>/linked` has for distribution, read-only (for
/// `doctor`): `linked` that is not a folder, and a `linked/<id>` of a read
/// folder that is not a link.
pub fn problems(linked: &Linked) -> Vec<String> {
    let dir = linked.dir();
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if !meta.is_dir() => return vec![blocked(dir, &meta)],
        Ok(_) => {}
        Err(_) => return Vec::new(),
    }
    linked
        .folders
        .iter()
        .filter(|f| matches!(f.state, State::Read(_)))
        .map(|f| (dir.join(&f.id), &f.id))
        .filter(|(path, _)| std::fs::symlink_metadata(path).is_ok_and(|m| !m.is_symlink()))
        .map(|(path, id)| occupied(&path, id))
        .collect()
}

/// `<canon>/linked` (`dir`, metadata `meta`) is a link or a file.
fn blocked(dir: &Path, meta: &std::fs::Metadata) -> String {
    let what = if meta.is_symlink() {
        "a link"
    } else {
        "a file"
    };
    format!(
        "{} is {what}, not a folder, so no linked skill is linked; move it away, then run `skiman sync`",
        dir.display()
    )
}

/// `<canon>/linked/<id>` (`path`) is not a link.
fn occupied(path: &Path, id: &str) -> String {
    format!(
        "{} is not a link, so linked folder `{id}` is not linked; move it away, then run `skiman sync`",
        path.display()
    )
}

/// Reports every linked folder that is not read in full ([`folder_items`]).
pub fn report_folders(linked: &Linked, kept: &BTreeMap<String, Holder>, report: &mut Report) {
    for folder in &linked.folders {
        report.items.extend(folder_items(linked, folder, kept));
    }
}

/// What sync and `doctor` report about `folder` when it is not read in
/// full: that it breaks a rule (exit 3), is not on this machine (exit 0,
/// keyed by its path, so a changed path is reported again), or cannot be
/// read (exit 3); or, when it was read, each skill whose `SKILL.md` cannot
/// be probed (exit 3) and each problem among its entries (exit unchanged).
/// While the folder is one skill only through `kept`
/// ([`Linked::is_single`]), its sub-folders are none of its skills or
/// entries: only its own `SKILL.md` is reported.
pub fn folder_items(
    linked: &Linked,
    folder: &Folder,
    kept: &BTreeMap<String, Holder>,
) -> Vec<Item> {
    let dir = linked.dir();
    let item = |path: PathBuf, what, message, attention| Item {
        scope: dir.display().to_string(),
        path,
        kind: ItemKind::Linked {
            what,
            message,
            attention,
        },
    };
    let id = &folder.id;
    let path = folder.path.clone().unwrap_or_else(|| dir.join(id));
    match &folder.state {
        State::Broken(why) => vec![item(
            path,
            "broken",
            format!("linked folder `{id}` is skipped: {why}; fix [linked.{id}] in skiman.toml"),
            true,
        )],
        State::Missing => vec![item(
            path.clone(),
            "missing",
            format!(
                "linked folder `{id}` ({}) is not on this machine, so its skills are not distributed",
                path.display()
            ),
            false,
        )],
        State::Unreadable(why) => vec![item(
            path,
            "unreadable",
            format!(
                "linked folder `{id}` is not read ({why}); its links stay as they are until a sync can read it"
            ),
            true,
        )],
        State::Read(contents) => {
            let children = !Linked::is_single(folder, kept);
            let top = contents
                .top_unprobed
                .iter()
                .map(|why| (id, why, path.clone()));
            let below = contents
                .unprobed
                .iter()
                .filter(|_| children)
                .map(|(name, why)| (name, why, folder.skill_dir(name)));
            let unprobed = top.chain(below).map(|(name, why, dir)| {
                item(
                    dir,
                    "unprobed",
                    format!(
                        "linked skill {name} of `{id}` is not checked ({why}); its links stay as they are"
                    ),
                    true,
                )
            });
            let problems = contents
                .problems
                .iter()
                .filter(|_| children)
                .map(|(entry, problem)| {
                    item(
                        entry.clone(),
                        "entry",
                        format!("{} {problem}", entry.display()),
                        false,
                    )
                });
            unprobed.chain(problems).collect()
        }
    }
}

/// The entries of `dir` and whether each is a link, sorted by name.
fn entries(dir: &Path) -> std::io::Result<Vec<(OsString, bool)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = std::fs::symlink_metadata(entry.path())?;
        out.push((entry.file_name(), meta.is_symlink()));
    }
    out.sort();
    Ok(out)
}

/// A report about `<canon>/linked` that needs attention (exit 3).
fn note(dir: &Path, path: PathBuf, what: &'static str, message: String) -> Item {
    Item {
        scope: dir.display().to_string(),
        path,
        kind: ItemKind::Linked {
            what,
            message,
            attention: true,
        },
    }
}
