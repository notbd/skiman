//! `skiman mv <old> <new>`: rename a skill. Renames `skills/<old>`, its
//! `SKILL.md` `name` line, `upstream/<old>`, the `[skills.<old>]` table of
//! `skiman.toml` (with `enabled` and `source`), the lock entry and the
//! `[[secrets.allow]]` paths, then syncs, which links the new name and
//! removes the old links. Nothing is backed up: every piece is renamed in
//! place. The folder goes last, so a mv stopped part-way leaves
//! `skills/<old>` with a `SKILL.md` that names `<new>`, which no other
//! command leaves; from there the same command finishes it.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::Exit;
use crate::backup::Backups;
use crate::change::{self, Change, Want};
use crate::commands::remove::check_single_name;
use crate::failure::{Failure, finish};
use crate::fsutil::read_text;
use crate::lockfile::{self, Lockfile};
use crate::manifest::{self, Manifest};
use crate::names;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::skill_md::SkillMd;
use crate::staging::Staging;
use crate::sync::{self, Output};
use crate::tree::{Kind, Overlay, Tree, WorkingTree};

pub fn run(old: &str, new: &str) -> Exit {
    finish(mv(old, new))
}

fn mv(old: &str, new: &str) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    check_single_name(old)?;
    names::check_new_name(new)?;

    let lock = RepoLock::wait(&paths, &repo)?;
    let folder =
        |name: &str| std::fs::symlink_metadata(repo.skill_dir(name)).is_ok_and(|m| m.is_dir());
    if !folder(old) {
        let done = match folder(new) {
            true => format!(
                "; skills/{new} is a skill folder: if `skiman mv {old} {new}` stopped part-way, it got through the skill repo, and `skiman sync` finishes the rest"
            ),
            false => String::new(),
        };
        return Err(Failure::refused(format!(
            "skills/{old} is not a skill folder{done}"
        )));
    }
    if half_renamed(&repo, old, new)? {
        out!("skills/{old}/SKILL.md already names `{new}`: finishing the rename");
    } else {
        names::check_not_taken(&repo, new, |_| Ok(false))?;
    }
    let plan = Plan::build(&repo, old, new)?;
    plan.validate(&repo)?;
    let change = plan.change();
    let held = change.held(repo.root())?;
    let mut backups = Backups::new(&paths);
    let staging = Staging::begin(&repo, &lock, &mut backups)?;
    let (done, result) = change.apply(&held, repo.root(), &staging, &mut backups);
    for line in &done {
        out!("{line}");
    }
    result.map_err(|failure| change::stopped(failure, &done))?;
    drop(staging);
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}

/// Whether the skill repo is where a mv of `old` to `new` stopped part-way:
/// `skills/<old>/SKILL.md` names `new` (the first thing mv writes),
/// `skills/<new>` is not there (the last), and each of the `skiman.toml`
/// table, the lock entry and `upstream/` has one of the two names at most
/// (each is written in one step).
fn half_renamed(repo: &SkillRepo, old: &str, new: &str) -> Result<bool, Failure> {
    let (manifest, lock) = (
        Manifest::load(repo)?.skills(),
        Lockfile::load(repo)?.skill_names(),
    );
    let has = |name: &str| {
        let upstream = std::fs::symlink_metadata(repo.upstream_dir(name)).is_ok();
        [manifest.contains_key(name), lock.contains(name), upstream]
    };
    let (old_has, new_has) = (has(old), has(new));
    let one_each = old_has.iter().zip(new_has).all(|(o, n)| !(*o && n));
    let skill_md = std::fs::read(repo.skill_dir(old).join("SKILL.md")).unwrap_or_default();
    let names_new =
        SkillMd::parse_for_rename("SKILL.md", &skill_md).is_ok_and(|s| s.name() == Some(new));
    let new_there = std::fs::symlink_metadata(repo.skill_dir(new)).is_ok();
    Ok(names_new && !new_there && one_each)
}

/// Every change `mv` makes, computed before anything is written.
struct Plan {
    old: String,
    new: String,
    /// The rewritten `SKILL.md`, when its `name` line changes.
    skill_md: Option<Vec<u8>>,
    has_upstream: bool,
    manifest: Option<String>,
    lock: Option<String>,
}

impl Plan {
    fn build(repo: &SkillRepo, old: &str, new: &str) -> Result<Plan, Failure> {
        let skill_md_path = repo.skill_dir(old).join("SKILL.md");
        let skill_md = match std::fs::symlink_metadata(&skill_md_path) {
            Ok(meta) if meta.is_file() => {
                let content = std::fs::read(&skill_md_path).map_err(|e| {
                    Failure::operational(format!("cannot read {}: {e}", skill_md_path.display()))
                })?;
                let parsed = SkillMd::parse_for_rename(&format!("skills/{old}/SKILL.md"), &content)
                    .map_err(|f| {
                        Failure::refused(format!(
                            "{}; fix it so skiman can rename the skill",
                            f.message
                        ))
                    })?;
                (parsed.name() != Some(new))
                    .then(|| parsed.with_name(new))
                    .transpose()?
                    .map(String::into_bytes)
            }
            _ => None,
        };
        let edit = |path: PathBuf,
                    rename: fn(&str, &str, &str) -> Result<String, String>|
         -> Result<Option<String>, Failure> {
            let Some(text) = read_text(&path)? else {
                return Ok(None);
            };
            let renamed = rename(&text, old, new)
                .map_err(|e| Failure::refused(format!("{}: {e}", path.display())))?;
            Ok((renamed != text).then_some(renamed))
        };
        Ok(Plan {
            old: old.to_owned(),
            new: new.to_owned(),
            skill_md,
            has_upstream: [old, new]
                .iter()
                .any(|name| std::fs::symlink_metadata(repo.upstream_dir(name)).is_ok()),
            manifest: edit(repo.manifest_file(), manifest::with_skill_renamed)?,
            lock: edit(repo.lockfile(), lockfile::with_entry_renamed)?,
        })
    }

    /// Moved content is checked only for the name rules of the new name
    /// (done before), so nothing here blocks; every violation of the skill
    /// repo as it will be is printed as a warning.
    fn validate(&self, repo: &SkillRepo) -> Result<(), Failure> {
        let working_tree = WorkingTree::new(repo.root());
        let io =
            |e: std::io::Error| Failure::operational(format!("cannot read the skill repo: {e}"));
        let mut files = Vec::new();
        let mut executable = BTreeSet::new();
        for root in ["skills", "upstream"] {
            // `upstream/` is renamed before the folder: a mv stopped part-way
            // may have done it.
            let at = |name: &str| format!("{root}/{name}");
            let from = match working_tree.kind(&at(&self.old)).map_err(io)? {
                Some(_) => at(&self.old),
                None => at(&self.new),
            };
            for entry in working_tree.walk(&from).map_err(io)? {
                let Kind::File { executable: x, .. } = entry.kind else {
                    continue;
                };
                let path = format!("{root}/{}{}", self.new, &entry.path[from.len()..]);
                let content = match &self.skill_md {
                    Some(renamed) if path == format!("skills/{}/SKILL.md", self.new) => {
                        renamed.clone()
                    }
                    _ => working_tree.read(&entry.path).map_err(io)?,
                };
                if x {
                    executable.insert(path.clone());
                }
                files.push((path, content));
            }
        }
        for (rel, text) in [
            (rules::MANIFEST, &self.manifest),
            (rules::LOCKFILE, &self.lock),
        ] {
            if let Some(text) = text {
                files.push((rel.to_owned(), text.clone().into_bytes()));
            }
        }
        let candidate = Overlay::new(&working_tree, files)
            .with_executable(executable)
            .replacing(vec![
                format!("skills/{}", self.old),
                format!("upstream/{}", self.old),
            ]);
        rules::validate_write(&candidate, &Scope::default())
    }

    /// The `SKILL.md` first (in `skills/<old>`), then `skiman.toml`,
    /// `skiman.lock`, the snapshot and, last, the folder.
    fn change(&self) -> Change {
        let (old, new) = (&self.old, &self.new);
        let mut pieces = Vec::new();
        if let Some(content) = &self.skill_md {
            let rel = format!("skills/{old}/SKILL.md");
            pieces.push((rel, Want::File(content.clone())));
        }
        for (rel, text) in [
            (rules::MANIFEST, &self.manifest),
            (rules::LOCKFILE, &self.lock),
        ] {
            if let Some(text) = text {
                pieces.push((rel.to_owned(), Want::File(text.clone().into_bytes())));
            }
        }
        for root in ["upstream", "skills"] {
            if root == "skills" || self.has_upstream {
                let from = Want::From(format!("{root}/{old}"));
                pieces.push((format!("{root}/{new}"), from));
            }
        }
        Change {
            pieces,
            command: "mv",
            save_files: false,
        }
    }
}
