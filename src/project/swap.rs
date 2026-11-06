//! The one protocol for every replace of a project item side (plan
//! "Writing"). Every temporary entry lives in a swap folder, one per
//! replaced item, on the destination's filesystem: `<repo>/.skiman/swap/<ts>/`
//! for the mirror, `<anchor>/.skiman-swap-<ts>/` for the project. Inside it,
//! `new/` holds the snapshot, `old/` the set-aside copy of what was there,
//! and `target` the destination path.
//!
//! A set-aside copy is only ever renamed, never copied and deleted: it keeps
//! its inode, so a late write through a handle opened before the swap lands
//! in it. Nothing replaced is deleted; an error never deletes the only copy.

use std::io;
use std::path::{Path, PathBuf};

use super::content::{self, Content};
use super::pattern::Kind;
use crate::backup::{Backup, timestamp};
use crate::fsutil;
use crate::paths::Paths;

/// Prefix of a project-side swap folder; discovery never enters one.
pub const PROJECT_PREFIX: &str = ".skiman-swap-";

/// Which side a swap folder serves, which decides where it lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// `<repo>/.skiman/swap/<ts>/`.
    Mirror,
    /// `<anchor>/.skiman-swap-<ts>/`, with a `.gitignore` holding `*`.
    Project,
}

/// One command's backup folder, created when first needed.
pub struct Backups<'a> {
    paths: &'a Paths,
    backup: Option<Backup>,
}

impl<'a> Backups<'a> {
    pub fn new(paths: &'a Paths) -> Backups<'a> {
        Backups {
            paths,
            backup: None,
        }
    }

    fn dir(&mut self) -> io::Result<PathBuf> {
        if self.backup.is_none() {
            self.backup = Some(Backup::create(self.paths)?);
        }
        Ok(self
            .backup
            .as_ref()
            .expect("just created")
            .dir()
            .to_path_buf())
    }

    /// The backup folder, when this command created one.
    pub fn created(&self) -> Option<&Path> {
        self.backup.as_ref().map(Backup::dir)
    }
}

/// What the plan saw at the destination of a replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// Nothing is there.
    Absent,
    /// An item side with this hash.
    Hash(String),
    /// Something that cannot be hashed as an item (a blocked project side):
    /// it is replaced as it is, and goes to the backup whole.
    Present,
}

impl Expect {
    pub fn from_hash(hash: Option<&str>) -> Expect {
        match hash {
            Some(hash) => Expect::Hash(hash.to_owned()),
            None => Expect::Absent,
        }
    }
}

/// How a replace ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The new content is in place. `backup` holds what was replaced;
    /// `late_write` is set when it changed while it was moved (a write
    /// through a handle opened before the swap landed in the backup).
    Replaced {
        backup: Option<PathBuf>,
        late_write: bool,
    },
    /// The destination was not what the plan saw; nothing was replaced.
    Conflict(String),
    /// Something stayed in a swap folder (an error, or a backup on another
    /// filesystem): what and where. Exit 4.
    Kept(String),
}

/// Replaces the item side at `dest` with `new`.
///
/// `expected` is what the plan saw at `dest`. `parent` is where the swap folder goes: `<repo>/.skiman/swap` for
/// [`Place::Mirror`], the item's anchor for [`Place::Project`]. What was
/// replaced goes to `<backup>/<backup_rel>`.
#[allow(clippy::too_many_arguments)]
pub fn replace(
    dest: &Path,
    kind: Kind,
    expected: &Expect,
    new: &Content,
    parent: &Path,
    place: Place,
    backups: &mut Backups,
    backup_rel: &Path,
) -> io::Result<Outcome> {
    let name = dest.file_name().expect("item path has a name").to_owned();
    let swap = create(parent, place, dest)?;
    let staged = swap.join("new").join(&name);
    let old = swap.join("old").join(&name);
    write(&staged, new)?;

    let present = match std::fs::symlink_metadata(dest) {
        Ok(_) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => {
            remove_swap(&swap);
            return Err(e);
        }
    };
    match (present, expected) {
        (false, Expect::Absent) => {}
        (false, _) => {
            remove_swap(&swap);
            return Ok(Outcome::Conflict(format!(
                "{} disappeared since the plan was made",
                dest.display()
            )));
        }
        (true, Expect::Absent) => {
            remove_swap(&swap);
            return Ok(Outcome::Conflict(format!(
                "{} appeared since the plan was made",
                dest.display()
            )));
        }
        (true, expected) => {
            std::fs::rename(dest, &old).inspect_err(|_| remove_swap(&swap))?;
            let seen = content::hash_of(&old, kind)?;
            let matches = match expected {
                Expect::Hash(hash) => seen.as_deref() == Some(hash.as_str()),
                _ => true,
            };
            if !matches {
                return Ok(match place_without_replacing(&old, dest, kind) {
                    Ok(()) => {
                        remove_swap(&swap);
                        Outcome::Conflict(format!(
                            "{} changed since the plan was made",
                            dest.display()
                        ))
                    }
                    Err(e) => Outcome::Kept(format!(
                        "{} changed since the plan was made, and something took its place before it could be put back ({e}); both are kept: {} and {}",
                        dest.display(),
                        dest.display(),
                        old.display()
                    )),
                });
            }
        }
    }

    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if let Err(e) = place_without_replacing(&staged, dest, kind) {
        if !present {
            remove_swap(&swap);
            return Ok(Outcome::Conflict(format!(
                "cannot put {} in place: {e}",
                dest.display()
            )));
        }
        return Ok(match place_without_replacing(&old, dest, kind) {
            Ok(()) => {
                remove_swap(&swap);
                Outcome::Kept(format!(
                    "cannot put the new {} in place ({e}); the previous one is back where it was",
                    dest.display()
                ))
            }
            Err(back) => Outcome::Kept(format!(
                "cannot put the new {} in place ({e}) nor put the previous one back ({back}); it is kept at {}",
                dest.display(),
                old.display()
            )),
        });
    }

    let mut outcome = Outcome::Replaced {
        backup: None,
        late_write: false,
    };
    if present {
        let before = fingerprint(&old)?;
        let moved = backups.dir().and_then(|dir| {
            let to = dir.join(backup_rel);
            std::fs::create_dir_all(to.parent().expect("backup path has a parent"))?;
            std::fs::rename(&old, &to)?;
            Ok(to)
        });
        match moved {
            Ok(to) => {
                let late_write = fingerprint(&to)? != before;
                outcome = Outcome::Replaced {
                    backup: Some(to),
                    late_write,
                };
            }
            Err(e) => {
                return Ok(Outcome::Kept(format!(
                    "{} is replaced, but the previous version cannot be moved to the backup ({e}); it is kept at {}",
                    dest.display(),
                    old.display()
                )));
            }
        }
    }
    remove_swap(&swap);
    Ok(outcome)
}

/// A hash of whatever is at `path`, links not followed (the tree hash of a
/// folder, the content hash of a file): what step 5 compares before and
/// after the move to the backup.
fn fingerprint(path: &Path) -> io::Result<String> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        crate::tree::tree_hash_of_dir(path)
    } else if meta.is_file() {
        Ok(Content::File(std::fs::read(path)?).hash())
    } else {
        Ok(String::new())
    }
}

/// Puts `from` at `to` without replacing anything that is there: a hard
/// link for a file (fails if `to` exists), `rename` for a folder (fails over
/// a file or a non-empty folder).
fn place_without_replacing(from: &Path, to: &Path, kind: Kind) -> io::Result<()> {
    match kind {
        Kind::File => {
            std::fs::hard_link(from, to)?;
            std::fs::remove_file(from)
        }
        Kind::Skill => std::fs::rename(from, to),
    }
}

/// Creates a fresh swap folder for `dest` under `parent`.
fn create(parent: &Path, place: Place, dest: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    let prefix = match place {
        Place::Mirror => "",
        Place::Project => PROJECT_PREFIX,
    };
    let stamp = timestamp();
    let dir = (1..)
        .map(|n| match n {
            1 => parent.join(format!("{prefix}{stamp}")),
            n => parent.join(format!("{prefix}{stamp}-{n}")),
        })
        .find_map(|dir| match std::fs::create_dir(&dir) {
            Ok(()) => Some(Ok(dir)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => None,
            Err(e) => Some(Err(e)),
        })
        .expect("the suffixes never run out")?;
    if place == Place::Project {
        std::fs::write(dir.join(".gitignore"), "*\n")?;
    }
    std::fs::write(dir.join("target"), dest.display().to_string())?;
    std::fs::create_dir(dir.join("new"))?;
    std::fs::create_dir(dir.join("old"))?;
    Ok(dir)
}

fn write(path: &Path, content: &Content) -> io::Result<()> {
    let failure = |f: crate::failure::Failure| io::Error::other(f.message);
    match content {
        Content::File(bytes) => fsutil::write_file(path, false, bytes).map_err(failure),
        Content::Skill(files) => fsutil::write_files(path, files).map_err(failure),
    }
}

/// Removes a swap folder whose `old/` is empty: `new/` is a copy of a
/// source that still exists, or a second link to an entry already in place.
fn remove_swap(swap: &Path) {
    if std::fs::read_dir(swap.join("old")).is_ok_and(|mut d| d.next().is_some()) {
        return;
    }
    let _ = std::fs::remove_dir_all(swap);
}

/// A swap folder an earlier command left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub dir: PathBuf,
    /// The destination it served, from its `target` file.
    pub target: Option<String>,
    /// Whether it still holds a set-aside copy (`old/` is not empty).
    pub holds_original: bool,
}

/// The swap folders directly under `parent` (`<repo>/.skiman/swap`, or an anchor).
pub fn leftovers(parent: &Path, place: Place) -> Vec<Leftover> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out: Vec<Leftover> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            place == Place::Mirror || e.file_name().to_string_lossy().starts_with(PROJECT_PREFIX)
        })
        .map(|e| {
            let dir = e.path();
            Leftover {
                target: std::fs::read_to_string(dir.join("target")).ok(),
                holds_original: std::fs::read_dir(dir.join("old"))
                    .is_ok_and(|mut d| d.next().is_some()),
                dir,
            }
        })
        .collect();
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// Moves what the swap folders under `parent` hold in `old/` into the
/// backup (`swap-leftovers/<folder>/<name>`), by rename only, then removes
/// the folders. Returns one message per folder; a copy that cannot be
/// renamed (another filesystem) stays, and its message says where.
pub fn recover(parent: &Path, place: Place, backups: &mut Backups) -> Vec<String> {
    let mut messages = Vec::new();
    for leftover in leftovers(parent, place) {
        let target = leftover
            .target
            .clone()
            .unwrap_or_else(|| "an item".to_owned());
        if !leftover.holds_original {
            remove_swap(&leftover.dir);
            continue;
        }
        let folder = leftover.dir.file_name().expect("named").to_owned();
        let moved = (|| -> io::Result<PathBuf> {
            let to = backups.dir()?.join("swap-leftovers").join(&folder);
            std::fs::create_dir_all(&to)?;
            for entry in std::fs::read_dir(leftover.dir.join("old"))? {
                let entry = entry?;
                std::fs::rename(entry.path(), to.join(entry.file_name()))?;
            }
            Ok(to)
        })();
        match moved {
            Ok(to) => {
                remove_swap(&leftover.dir);
                messages.push(format!(
                    "moved the previous version of {target}, left by an interrupted write in {}, to {}",
                    leftover.dir.display(),
                    to.display()
                ));
            }
            Err(e) => messages.push(format!(
                "{} holds the previous version of {target} from an interrupted write and cannot be moved to the backup ({e}); move it by hand",
                leftover.dir.display()
            )),
        }
    }
    messages
}
