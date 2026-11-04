//! `.skiman/tmp/`: where write commands stage content before renaming it
//! into the skill repo (spec Data formats, Skill repo layout; clarifications
//! 3, 4 and 6 on #1).
//!
//! Anything in `tmp/` that no running command staged is a **tmp leftover**
//! (distinct from "leftover records", exit 3). [`Staging::begin`] moves tmp
//! leftovers to a fresh backup, then creates the command's own
//! `tmp/<timestamp>/`, which is removed when the [`Staging`] is dropped.

use std::io;
use std::path::{Path, PathBuf};

use crate::backup::{self, Backup};
use crate::failure::Failure;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;

/// A command's own `tmp/<timestamp>/` folder (disposable staging).
#[derive(Debug)]
pub struct Staging {
    dir: PathBuf,
}

impl Staging {
    /// Call while holding the repo lock, after every check that can refuse
    /// the command without writing (exit 1 or 2), and before its first write.
    /// Moves tmp leftovers to `backups/<timestamp>/repo/.skiman/tmp/` and
    /// prints that path; if the move fails, stops with exit 4 naming the path
    /// to move by hand. tmp leftovers are never deleted.
    pub fn begin(paths: &Paths, repo: &SkillRepo, _lock: &RepoLock) -> Result<Staging, Failure> {
        let tmp = repo.tmp_dir();
        move_tmp_leftovers(paths, &tmp)?;
        let dir = backup::fresh_dir(&tmp).map_err(|e| {
            Failure::operational(format!("cannot create a folder in {}: {e}", tmp.display()))
        })?;
        Ok(Staging { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn move_tmp_leftovers(paths: &Paths, tmp: &Path) -> Result<(), Failure> {
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
    let stop = |e: io::Error, path: &Path| {
        Failure::operational(format!(
            "cannot move tmp leftovers to a backup: {e}; move {} out of the skill repo by hand, then run the command again",
            path.display()
        ))
    };
    let backup = Backup::create(paths).map_err(|e| stop(e, tmp))?;
    let rel = Path::new("repo/.skiman/tmp");
    for leftover in &leftovers {
        let name = leftover.file_name().expect("entry has a name");
        backup
            .move_in(leftover, &rel.join(name))
            .map_err(|e| stop(e, leftover))?;
    }
    println!(
        "moved tmp leftovers to {}",
        backup.dir().join(rel).display()
    );
    Ok(())
}

fn list(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        out.push(entry?.path());
    }
    out.sort();
    Ok(out)
}

/// Renames `from` (staged under `tmp/`) to `to` in the skill repo. A rename
/// across filesystems fails with exit 4 and never falls back to copying.
pub fn rename_into_place(from: &Path, to: &Path) -> Result<(), Failure> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(Failure::operational(format!(
            "{} appeared while skiman was writing; nothing was put in its place",
            to.display()
        )));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            Failure::operational(format!("cannot create {}: {e}", parent.display()))
        })?;
    }
    std::fs::rename(from, to).map_err(|e| {
        if e.kind() == io::ErrorKind::CrossesDevices {
            Failure::operational(format!(
                "cannot rename {} to {}: they are on different filesystems (skiman never copies instead); keep .skiman/ on the skill repo's filesystem",
                from.display(),
                to.display()
            ))
        } else {
            Failure::operational(format!(
                "cannot rename {} to {}: {e}",
                from.display(),
                to.display()
            ))
        }
    })
}
