//! `skiman remove <name>`: move `skills/<name>` and `upstream/<name>` (each
//! when present) to a fresh backup, then remove the skill's `skiman.toml`
//! table and lock entry, then sync (which removes its links). Also cleans up
//! the leftover records of a hand-deleted skill.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup::Backup;
use crate::commands::{confirm, has_terminal};
use crate::failure::{Failure, finish};
use crate::fsutil::write_atomic;
use crate::lockfile::Lockfile;
use crate::manifest::{self, Manifest};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::sync::{self, Output};
use crate::tree::{Overlay, WorkingTree};

pub fn run(name: &str, yes: bool) -> Exit {
    finish(remove(name, yes))
}

fn remove(name: &str, yes: bool) -> Result<Exit, Failure> {
    if !yes && !has_terminal() {
        return Err(Failure::usage("missing flag without a terminal: --yes"));
    }
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    check_single_name(name)?;
    let found = Found::read(&repo, name)?;
    if !yes {
        found.print(&repo);
        if !confirm(&format!("Remove skill {name}? [y/N] "))? {
            return Err(Failure::refused("not removed; nothing was changed"));
        }
    }

    let lock = RepoLock::wait(&repo)?;
    let found = Found::read(&repo, name)?;
    let records = Records::build(&repo, name, &found)?;
    validate(&repo, name, &records)?;
    let backup = move_folders(&paths, &found)?;
    records.write()?;
    match backup {
        Some(dir) => out!("removed skill {name}; its folders are in {}", dir.display()),
        None => out!("removed the records of skill {name}"),
    }
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}

/// Refuses (exit 1) a name that is not one plain path component, so the
/// folders `remove` moves are always inside `skills/` and `upstream/`.
pub(crate) fn check_single_name(name: &str) -> Result<(), Failure> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(Failure::refused(format!("`{name}` is not a skill name")));
    }
    Ok(())
}

/// What exists of the skill: its folders (any kind of entry) and records.
struct Found {
    /// `(path, repo-relative path)` of `skills/<name>` and `upstream/<name>`.
    folders: Vec<(PathBuf, String)>,
    table: bool,
    lock_entry: bool,
}

impl Found {
    /// Refuses (exit 1) a name with neither a folder nor any record.
    fn read(repo: &SkillRepo, name: &str) -> Result<Found, Failure> {
        let folders: Vec<(PathBuf, String)> = [
            (repo.skill_dir(name), format!("skills/{name}")),
            (repo.upstream_dir(name), format!("upstream/{name}")),
        ]
        .into_iter()
        .filter(|(path, _)| std::fs::symlink_metadata(path).is_ok())
        .collect();
        let table = Manifest::load(repo)?.skills().contains_key(name);
        let lock_entry = Lockfile::load(repo)?.skill_names().contains(name);
        if folders.is_empty() && !table && !lock_entry {
            return Err(Failure::refused(format!(
                "there is no skill {name}: no skills/{name}, no upstream/{name}, no [skills.{name}] table in skiman.toml and no entry in skiman.lock"
            )));
        }
        Ok(Found {
            folders,
            table,
            lock_entry,
        })
    }

    fn print(&self, repo: &SkillRepo) {
        for (path, _) in &self.folders {
            out!("move to a backup: {}", path.display());
        }
        if self.table {
            out!("remove from {}: its table", repo.manifest_file().display());
        }
        if self.lock_entry {
            out!("remove from {}: its entry", repo.lockfile().display());
        }
    }
}

/// The new `skiman.toml` and `skiman.lock` texts, for the files that change.
struct Records {
    manifest: Option<(PathBuf, String)>,
    lock: Option<(PathBuf, String)>,
}

impl Records {
    fn build(repo: &SkillRepo, name: &str, found: &Found) -> Result<Records, Failure> {
        let edit = |path: PathBuf, needed: bool| -> Result<Option<(PathBuf, String)>, Failure> {
            if !needed {
                return Ok(None);
            }
            let text = std::fs::read_to_string(&path).map_err(|e| {
                Failure::operational(format!("cannot read {}: {e}", path.display()))
            })?;
            let text = manifest::without_skill(&text, name)
                .map_err(|e| Failure::refused(format!("{}: {e}", path.display())))?;
            Ok(Some((path, text)))
        };
        Ok(Records {
            manifest: edit(repo.manifest_file(), found.table)?,
            lock: edit(repo.lockfile(), found.lock_entry)?,
        })
    }

    /// `skiman.toml` first, then `skiman.lock`.
    fn write(&self) -> Result<(), Failure> {
        for (path, text) in self.manifest.iter().chain(&self.lock) {
            write_atomic(path, text.as_bytes()).map_err(|e| {
                Failure::operational(format!(
                    "cannot write {}: {e}; the skill's folders are already in the backup",
                    path.display()
                ))
            })?;
            out!("changed {}", path.display());
        }
        Ok(())
    }
}

/// The rules on the skill repo as it will be: the removed skill must leave
/// nothing inconsistent; everything else is a warning.
fn validate(repo: &SkillRepo, name: &str, records: &Records) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(repo.root());
    let texts = [
        (rules::MANIFEST, &records.manifest),
        (rules::LOCKFILE, &records.lock),
    ]
    .into_iter()
    .filter_map(|(rel, edit)| {
        edit.as_ref()
            .map(|(_, text)| (rel.to_owned(), text.clone().into_bytes()))
    })
    .collect();
    let candidate = Overlay::new(&working_tree, texts)
        .replacing(vec![format!("skills/{name}"), format!("upstream/{name}")]);
    let scope = Scope {
        consistency: BTreeSet::from([name.to_owned()]),
        ..Scope::default()
    };
    rules::validate_write(&candidate, &scope)
}

/// Moves every folder of the skill into one fresh backup; `None` when there
/// is no folder. A failure stops before any record is removed.
fn move_folders(paths: &Paths, found: &Found) -> Result<Option<PathBuf>, Failure> {
    if found.folders.is_empty() {
        return Ok(None);
    }
    let backup = Backup::create(paths).map_err(|e| {
        Failure::operational(format!(
            "cannot create a backup folder in {}: {e}; nothing was removed",
            paths.state_dir().join("backups").display()
        ))
    })?;
    for (path, rel) in &found.folders {
        let dest = backup
            .move_in(path, &Path::new("repo").join(rel))
            .map_err(|e| {
                Failure::operational(format!(
                    "cannot move {} to the backup {}: {e}; the skill's records were not removed",
                    path.display(),
                    backup.dir().display()
                ))
            })?;
        out!("moved {} to {}", path.display(), dest.display());
    }
    Ok(Some(backup.dir().to_path_buf()))
}
