//! `skiman remove <name>`: move the skill folder (`skills/[<group>/]<name>`)
//! and `upstream/<name>` (each when present) to a fresh backup, then remove
//! the skill's `skiman.toml` table and secret exemptions and its lock entry,
//! then sync (which removes its links). A group folder the skill leaves
//! empty is removed. Also cleans up the leftover records of a hand-deleted
//! skill. A remove stopped part-way finishes when it is run again.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup::Backups;
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::change::{Change, Want};
use crate::commands::{confirm, has_terminal};
use crate::failure::{Failure, finish};
use crate::fsutil::read_text;
use crate::linked::Linked;
use crate::lockfile::Lockfile;
use crate::manifest::{self, Manifest};
use crate::paths::Paths;
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
    let repo = Canon::from_config(&paths)?;
    check_single_name(name)?;
    let found = Found::read(paths.home(), &repo, name)?;
    if !yes {
        found.print(&repo);
        if !confirm(&format!("Remove skill {name}? [y/N] "))? {
            return Err(Failure::refused("not removed; nothing was changed"));
        }
    }

    let lock = CanonLock::wait(&paths, &repo)?;
    Found::read(paths.home(), &repo, name)?;
    let skill = repo.layout()?.path(name);
    let change = change(&repo, &skill, name)?;
    validate(&repo, name, &change)?;
    let mut backups = Backups::new(&paths);
    let backed_up = change.apply(&repo, &lock, &mut backups)?;
    match backups.created().filter(|_| backed_up) {
        Some(dir) => out!("removed skill {name}; its folders are in {}", dir.display()),
        None => out!("removed the records of skill {name}"),
    }
    repo.remove_empty_group(&skill);
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
    /// `(path, repo-relative path)` of the skill folder and `upstream/<name>`.
    folders: Vec<(PathBuf, String)>,
    table: bool,
    lock_entry: bool,
}

impl Found {
    /// Refuses (exit 1) a name with neither a folder nor any record (naming
    /// the linked folder of a linked skill of that name), and the name of a
    /// group folder.
    fn read(home: &Path, repo: &Canon, name: &str) -> Result<Found, Failure> {
        let layout = repo.layout()?;
        if layout.find(name).is_none()
            && let Some(group) = layout.group_named(name)
        {
            return Err(Failure::refused(format!(
                "skills/{group} is a group folder, not a skill; remove its skills one by one"
            )));
        }
        let skill = layout.path(name);
        let folders: Vec<(PathBuf, String)> = [
            (repo.root().join(&skill), skill),
            (repo.upstream_dir(name), format!("upstream/{name}")),
        ]
        .into_iter()
        .filter(|(path, _)| std::fs::symlink_metadata(path).is_ok())
        .collect();
        let table = Manifest::load(repo)?.skills().contains_key(name);
        let lock_entry = Lockfile::load(repo)?.skill_names().contains(name);
        if folders.is_empty() && !table && !lock_entry {
            let linked = Linked::load(home, repo)?;
            return Err(Failure::refused(match linked.edited_elsewhere(name) {
                Some((why, id)) => format!(
                    "{why}; to stop distributing linked folder `{id}`, run `skiman linked remove {id}`"
                ),
                None => format!(
                    "there is no skill {name}: no skills/{name}, no upstream/{name}, no [skills.{name}] table in skiman.toml and no entry in skiman.lock"
                ),
            }));
        }
        Ok(Found {
            folders,
            table,
            lock_entry,
        })
    }

    fn print(&self, repo: &Canon) {
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
fn change(repo: &Canon, skill: &str, name: &str) -> Result<Change, Failure> {
    let mut pieces = vec![
        (skill.to_owned(), Want::Absent),
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

/// The rules on the canon as it will be: the removed skill must leave
/// nothing inconsistent; everything else is a warning.
fn validate(repo: &Canon, name: &str, change: &Change) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(repo.root());
    let scope = Scope {
        consistency: BTreeSet::from([name.to_owned()]),
        ..Scope::default()
    };
    rules::validate_write(&change.overlay(&working_tree), &scope)
}
