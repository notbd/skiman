//! `skiman remove <name>`: move `skills/<name>` and `upstream/<name>` (each
//! when present) to a fresh backup, then remove the skill's `skiman.toml`
//! table and secret exemptions and its lock entry, then sync (which removes
//! its links). Also cleans up the leftover records of a hand-deleted skill.
//! A remove stopped part-way finishes when it is run again.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::Exit;
use crate::backup::Backups;
use crate::change::{Change, Want};
use crate::commands::{confirm, has_terminal};
use crate::failure::{Failure, finish};
use crate::fsutil::read_text;
use crate::lockfile::Lockfile;
use crate::manifest::{self, Manifest};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::sync::{self, Output};
use crate::tree::WorkingTree;

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

    let lock = RepoLock::wait(&paths, &repo)?;
    Found::read(&repo, name)?;
    let change = change(&repo, name)?;
    validate(&repo, name, &change)?;
    let mut backups = Backups::new(&paths);
    let backed_up = change.apply(&repo, &lock, &mut backups)?;
    match backups.created().filter(|_| backed_up) {
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

/// Both folders absent, then `skiman.toml` without the skill's table and
/// secret exemptions, then `skiman.lock` without its entry.
fn change(repo: &SkillRepo, name: &str) -> Result<Change, Failure> {
    let mut pieces = vec![
        (format!("skills/{name}"), Want::Absent),
        (format!("upstream/{name}"), Want::Absent),
    ];
    for (rel, path) in [
        (rules::MANIFEST, repo.manifest_file()),
        (rules::LOCKFILE, repo.lockfile()),
    ] {
        if let Some(text) = read_text(&path)? {
            let text = manifest::without_skill(&text, name)
                .map_err(|e| Failure::refused(format!("{}: {e}", path.display())))?;
            pieces.push((rel.to_owned(), Want::File(text.into_bytes())));
        }
    }
    Ok(Change {
        pieces,
        command: "remove",
        save_files: false,
    })
}

/// The rules on the skill repo as it will be: the removed skill must leave
/// nothing inconsistent; everything else is a warning.
fn validate(repo: &SkillRepo, name: &str, change: &Change) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(repo.root());
    let scope = Scope {
        consistency: BTreeSet::from([name.to_owned()]),
        ..Scope::default()
    };
    rules::validate_write(&change.overlay(&working_tree), &scope)
}
