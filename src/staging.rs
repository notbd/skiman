//! `.skiman/tmp/`: where write commands stage content before renaming it
//! into the skill repo (spec Data formats, Skill repo layout; clarifications
//! 3, 4 and 6 on #1).
//!
//! Anything in `tmp/` that no running command staged is a **tmp leftover**
//! (distinct from "leftover records", exit 3). [`Staging::begin`] moves tmp
//! leftovers to the command's backup, then creates the command's own
//! `tmp/<timestamp>/`, which is removed when the [`Staging`] is dropped,
//! except for what [`crate::change`] swapped out that is still in `old/`.

use std::io;
use std::path::{Path, PathBuf};

use crate::backup::{self, Backups};
use crate::failure::Failure;
use crate::fsutil;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;

/// A command's own `tmp/<timestamp>/` folder (disposable staging).
#[derive(Debug)]
pub struct Staging {
    dir: PathBuf,
    /// The skill repo's root.
    root: PathBuf,
}

impl Staging {
    /// Call while holding the repo lock, after every check that can refuse
    /// the command without writing (exit 1 or 2), and before its first write.
    /// Moves tmp leftovers to `<backup>/repo/.skiman/tmp/` and prints that
    /// path; if the move fails, stops with exit 4 naming the path to move by
    /// hand. tmp leftovers are never deleted. Refuses (exit 4) a
    /// `.skiman/tmp/` reached through a link.
    pub fn begin(
        repo: &SkillRepo,
        _lock: &RepoLock,
        backups: &mut Backups,
    ) -> Result<Staging, Failure> {
        let tmp = repo.tmp_dir();
        let cannot = |why: String| {
            Failure::operational(format!(
                "cannot create a folder in {}: {why}",
                tmp.display()
            ))
        };
        if let Some(why) = fsutil::broken_chain(repo.root(), &tmp.join("new")) {
            return Err(cannot(why));
        }
        move_tmp_leftovers(backups, &tmp)?;
        let dir = backup::fresh_dir(&tmp).map_err(|e| cannot(e.to_string()))?;
        Ok(Staging {
            dir,
            root: repo.root().to_owned(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `tmp/<timestamp>/old/`: where a folder that is replaced is swapped
    /// out to before it goes to the backup. Never disposable.
    pub fn old_dir(&self) -> PathBuf {
        self.dir.join("old")
    }

    /// Renames `from` (staged under `tmp/`, or an entry of the skill repo
    /// that `mv` renames) to `to`, where nothing may be: an entry that
    /// appeared there is never replaced ([`fsutil::place_new_or_rename`]).
    /// Every folder of the skill repo on the way to `to` must be a real
    /// folder. Fails with exit 4, never copying instead.
    pub fn place(&self, from: &Path, to: &Path) -> Result<(), Failure> {
        let fail = |why: String| {
            Failure::operational(format!(
                "cannot rename {} to {}: {why}",
                from.display(),
                to.display()
            ))
        };
        if let Some(why) = fsutil::broken_chain(&self.root, to) {
            return Err(fail(why));
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Failure::operational(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        fsutil::publish(from, to).map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => Failure::operational(format!(
                "{} appeared while skiman was writing; nothing was put in its place",
                to.display()
            )),
            io::ErrorKind::CrossesDevices => Failure::operational(fsutil::crossing(from, to)),
            _ => fail(e.to_string()),
        })
    }
}

/// Removes the disposable staging, then every empty folder in `tmp/<ts>`
/// and `tmp/<ts>` itself once empty. What is still in `old/` stays (with the
/// folders above it) as a tmp leftover; only its empty folders go.
impl Drop for Staging {
    fn drop(&mut self) {
        let old = self.old_dir();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path == old {
                    continue;
                }
                let _ = match entry.file_type() {
                    Ok(t) if t.is_dir() => std::fs::remove_dir_all(&path),
                    _ => std::fs::remove_file(&path),
                };
            }
        }
        fsutil::remove_empty_dirs(&self.dir);
    }
}

fn move_tmp_leftovers(backups: &mut Backups, tmp: &Path) -> Result<(), Failure> {
    let leftovers = match list(tmp) {
        Ok(leftovers) => leftovers,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(Failure::operational(format!(
                "cannot read {}: {e}",
                tmp.display()
            )));
        }
    };
    if leftovers.is_empty() {
        return Ok(());
    }
    let by_hand = |path: &Path| {
        format!(
            "move {} out of the skill repo by hand, then run the command again",
            path.display()
        )
    };
    let by_hand_tmp = by_hand(tmp);
    let dir = backups
        .dir()
        .map_err(|e| Failure::operational(format!("{e}; {by_hand_tmp}")))?;
    let rel = Path::new("repo/.skiman/tmp");
    for leftover in &leftovers {
        let name = leftover.file_name().expect("entry has a name");
        backups.move_in(leftover, &rel.join(name)).map_err(|e| {
            Failure::operational(format!(
                "cannot move tmp leftovers to a backup: {e}; {}",
                by_hand(leftover)
            ))
        })?;
    }
    out!("moved tmp leftovers to {}", dir.join(rel).display());
    Ok(())
}

/// The entries of `dir`, sorted by path.
pub fn list(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        out.push(entry?.path());
    }
    out.sort();
    Ok(out)
}
