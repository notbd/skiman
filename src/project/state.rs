//! The state table (plan "Sync: the state table"), rows 4 to 15, for one
//! item. Rows 1 to 3 (the skill repo mid-operation, an unusable root, a
//! mirror path with no item shape) are decided by the callers before an item
//! is evaluated. The same classifier serves sync, `adopt`, `restore` and
//! `doctor`, so every command agrees on what an item's state is.

use std::path::Path;

use super::content::{self, Content, Side};
use super::item::Item;
use super::root::Root;

/// One row of the state table, or one of the two states outside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Row 4: the mirror is gone and the project holds no item: forget the synced hash.
    Forgotten,
    /// Row 5: the mirror entry cannot be used as it is: why.
    MirrorInvalid(String),
    /// Row 6: the item's project folder is gone.
    ProjectGone,
    /// Row 7: the project folder is not on this machine and never was.
    NotHere,
    /// Row 8: something else is at the item's path or on the way to it: what.
    Occupied(String),
    /// Row 9: the mirror holds it, this machine never had it.
    NotOnMachine,
    /// Row 10: deleted in the project since the last sync.
    DeletedInProject,
    /// Row 11: deleted in the mirror since the last sync.
    DeletedInMirror,
    /// Row 12: both sides are equal.
    InSync,
    /// Row 13: the project changed, the mirror did not.
    ProjectChanged,
    /// Row 13, blocked: the project side cannot be taken as it is (why).
    Blocked(String),
    /// Row 14: the mirror changed, the project did not.
    MirrorChanged,
    /// Row 15: both changed, or both differ with nothing recorded.
    Conflict,
    /// On disk only, never adopted: a candidate for `adopt`.
    New,
    /// On neither side, nothing recorded.
    Nothing,
}

impl State {
    /// Whether the state needs the user (exit 3).
    pub fn needs_attention(&self) -> bool {
        matches!(
            self,
            State::MirrorInvalid(_)
                | State::Occupied(_)
                | State::DeletedInProject
                | State::DeletedInMirror
                | State::Blocked(_)
                | State::MirrorChanged
                | State::Conflict
        )
    }
}

/// An item's state, with what was read on the way.
#[derive(Debug, Clone)]
pub struct Evaluated {
    pub state: State,
    /// The project side, when it was read.
    pub disk: Option<Content>,
    pub disk_hash: Option<String>,
    pub mirror_hash: Option<String>,
}

/// Evaluates `item` of `root`; `repo_root` is the skill repo's realpath and
/// `synced` the item's synced hash (`None` when nothing is recorded).
pub fn evaluate(
    root: &Root,
    item: &Item,
    repo_root: &Path,
    synced: Option<&str>,
) -> std::io::Result<Evaluated> {
    let mirror = content::read(&repo_root.join(item.mirror_rel()), item.kind)?;
    let mirror_hash = match &mirror {
        Side::Present(c) => Some(c.hash()),
        _ => None,
    };
    let disk = read_disk(root, item)?;
    let done = |state: State, disk: Option<Content>| {
        let disk_hash = disk.as_ref().map(Content::hash);
        Ok(Evaluated {
            state,
            disk,
            disk_hash,
            mirror_hash: mirror_hash.clone(),
        })
    };
    let disk_is_item = matches!(
        disk,
        Disk::Side(Side::Present(_)) | Disk::Side(Side::Unsafe(_))
    );
    if mirror == Side::Absent && synced.is_some() && !disk_is_item {
        return done(State::Forgotten, None);
    }
    match &mirror {
        Side::WrongKind => {
            return done(
                State::MirrorInvalid("it is not a regular file or folder".to_owned()),
                None,
            );
        }
        Side::Unsafe(why) => return done(State::MirrorInvalid(why.clone()), None),
        _ => {}
    }
    let disk = match disk {
        Disk::NoAnchor if synced.is_some() => return done(State::ProjectGone, None),
        Disk::NoAnchor => return done(State::NotHere, None),
        Disk::Occupied(what) => return done(State::Occupied(what), None),
        Disk::Side(Side::WrongKind) => {
            return done(
                State::Occupied(format!(
                    "{} is not a {}",
                    item.disk(root).display(),
                    match item.kind {
                        super::pattern::Kind::File => "regular file",
                        super::pattern::Kind::Skill => "real folder",
                    }
                )),
                None,
            );
        }
        Disk::Side(Side::Unsafe(why)) => return done(State::Blocked(why), None),
        Disk::Side(Side::Absent) => None,
        Disk::Side(Side::Present(content)) => Some(content),
    };
    let state = match (&disk, &mirror_hash, synced) {
        (None, Some(_), None) => State::NotOnMachine,
        (None, Some(_), Some(_)) => State::DeletedInProject,
        (None, None, _) => State::Nothing,
        (Some(_), None, Some(_)) => State::DeletedInMirror,
        (Some(_), None, None) => State::New,
        (Some(d), Some(m), s) => {
            let d = d.hash();
            if &d == m {
                State::InSync
            } else if s == Some(m.as_str()) {
                State::ProjectChanged
            } else if s == Some(d.as_str()) {
                State::MirrorChanged
            } else {
                State::Conflict
            }
        }
    };
    done(state, disk)
}

/// The project side of an item, as far as the state table needs it.
enum Disk {
    /// The item's project folder does not exist.
    NoAnchor,
    /// A path component between the root and the item exists but is not a real folder.
    Occupied(String),
    Side(Side),
}

fn read_disk(root: &Root, item: &Item) -> std::io::Result<Disk> {
    if std::fs::symlink_metadata(item.anchor_dir(root)).is_err() {
        return Ok(Disk::NoAnchor);
    }
    let mut dir = root.dir.clone();
    let parents: Vec<&str> = item.rel.split('/').collect();
    for part in &parents[..parents.len() - 1] {
        dir.push(part);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Ok(Disk::Occupied(format!(
                    "{} is not a real folder (a link or a file is in the way)",
                    dir.display()
                )));
            }
            // A missing intermediate folder means the item is absent.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Disk::Side(Side::Absent));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Disk::Side(content::read(&item.disk(root), item.kind)?))
}
