//! `skiman mv <old> [<new>] [--group <group> | --no-group]`: rename a skill,
//! move it to another group folder, or both. A rename renames the skill
//! folder, its `SKILL.md` `name` line, `upstream/<old>`, the `[skills.<old>]`
//! table of `skiman.toml` (with `enabled` and `source`), the lock entry and
//! the `[[secrets.allow]]` paths; a move to another group renames only the
//! skill folder, since no record names a group (ADR 0008). Then it syncs,
//! which links the new name and removes the old links, or retargets the
//! links of a moved skill. Nothing is backed up: every piece is renamed in
//! place. The folder goes last, so a rename stopped part-way leaves the old
//! folder with a `SKILL.md` that names `<new>`, which no other command
//! leaves; from there the same command finishes it. A group folder the
//! skill leaves empty is removed.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::Exit;
use crate::backup::Backups;
use crate::change::{Change, Want};
use crate::commands::remove::check_single_name;
use crate::failure::{Failure, finish};
use crate::fsutil::read_text;
use crate::layout::skill_path;
use crate::lockfile::{self, Lockfile};
use crate::manifest::{self, Manifest};
use crate::names;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::skill_md::SkillMd;
use crate::sync::{self, Output};
use crate::tree::{Kind, Overlay, Tree, WorkingTree};

/// The group folder `mv` moves a skill to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum To {
    /// The group it is in now.
    Same,
    Group(String),
    /// Out of any group: `skills/<name>`.
    NoGroup,
}

/// `new` is the new name; `None` keeps the name.
pub fn run(old: &str, new: Option<&str>, to: To) -> Exit {
    finish(mv(old, new.unwrap_or(old), to))
}

fn mv(old: &str, new: &str, to_arg: To) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    check_single_name(old)?;
    names::check_new_name(new)?;

    let lock = RepoLock::wait(&paths, &repo)?;
    let layout = repo.layout()?;
    let folder = |name: &str| {
        let placed = layout.find(name)?;
        let is_dir =
            std::fs::symlink_metadata(repo.root().join(placed.path())).is_ok_and(|m| m.is_dir());
        is_dir.then_some(placed)
    };
    let Some(from) = folder(old) else {
        let done = match folder(new) {
            Some(placed) if new != old => format!(
                "; {} is a skill folder: if `skiman mv {old} {new}` stopped part-way, it got through the skill repo, and `skiman sync` finishes the rest",
                placed.path()
            ),
            _ => String::new(),
        };
        return Err(Failure::refused(format!(
            "{} is not a skill folder{done}",
            layout.path(old)
        )));
    };
    let group = match &to_arg {
        To::Same => from.group.clone(),
        To::Group(group) => {
            names::check_group(group, &layout)?;
            Some(group.clone())
        }
        To::NoGroup => None,
    };
    let (from, to) = (from.path(), skill_path(group.as_deref(), new));
    if from == to {
        // `mv pdf pdf` names a taken name; a move to the group it is in
        // finishes a move that is done but not yet synced.
        if to_arg == To::Same {
            names::check_not_taken(&repo, new, |_| Ok(false))?;
        }
        out!("{to} is already in place");
        return Ok(sync::run(&paths, &repo, &lock, Output::Normal));
    }
    if new != old {
        if half_renamed(&repo, &from, old, new)? {
            out!("{from}/SKILL.md already names `{new}`: finishing the rename");
        } else {
            names::check_not_taken(&repo, new, |_| Ok(false))?;
        }
    }
    let plan = Plan::build(&repo, old, new, from, to)?;
    plan.validate(&repo)?;
    let change = plan.change();
    change.apply(&repo, &lock, &mut Backups::new(&paths))?;
    repo.remove_empty_group(&plan.from);
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}

/// Whether the skill repo is where a rename of `old` (in the skill folder
/// `from`) to `new` stopped part-way: `from/SKILL.md` names `new` (the first
/// thing mv writes), no skill folder of `new` is there (the last), and each
/// of the `skiman.toml` table, the lock entry and `upstream/` has one of the
/// two names at most (each is written in one step).
fn half_renamed(repo: &SkillRepo, from: &str, old: &str, new: &str) -> Result<bool, Failure> {
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
    let skill_md = std::fs::read(repo.root().join(from).join("SKILL.md")).unwrap_or_default();
    let names_new =
        SkillMd::parse_for_rename("SKILL.md", &skill_md).is_ok_and(|s| s.name() == Some(new));
    let new_there = repo.layout()?.find(new).is_some();
    Ok(names_new && !new_there && one_each)
}

/// Every change `mv` makes, computed before anything is written.
struct Plan {
    old: String,
    new: String,
    /// The skill folder now, and where it goes.
    from: String,
    to: String,
    /// The rewritten `SKILL.md`, when its `name` line changes.
    skill_md: Option<Vec<u8>>,
    has_upstream: bool,
    manifest: Option<String>,
    lock: Option<String>,
}

impl Plan {
    fn build(
        repo: &SkillRepo,
        old: &str,
        new: &str,
        from: String,
        to: String,
    ) -> Result<Plan, Failure> {
        let skill_md_path = repo.root().join(&from).join("SKILL.md");
        let skill_md = match std::fs::symlink_metadata(&skill_md_path) {
            Ok(meta) if meta.is_file() => {
                let content = std::fs::read(&skill_md_path).map_err(|e| {
                    Failure::operational(format!("cannot read {}: {e}", skill_md_path.display()))
                })?;
                let parsed = SkillMd::parse_for_rename(&format!("{from}/SKILL.md"), &content)
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
            from,
            to,
            skill_md,
            has_upstream: old != new
                && [old, new]
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
        let upstream = |name: &str| format!("upstream/{name}");
        let mut moves = vec![(self.from.clone(), self.to.clone())];
        if self.has_upstream {
            moves.push((upstream(&self.old), upstream(&self.new)));
        }
        for (old, new) in moves {
            // `upstream/` is renamed before the folder: a mv stopped part-way
            // may have done it.
            let from = match working_tree.kind(&old).map_err(io)? {
                Some(_) => old,
                None => new.clone(),
            };
            for entry in working_tree.walk(&from).map_err(io)? {
                let Kind::File { executable: x, .. } = entry.kind else {
                    continue;
                };
                let path = format!("{new}{}", &entry.path[from.len()..]);
                let content = match &self.skill_md {
                    Some(renamed) if path == format!("{}/SKILL.md", self.to) => renamed.clone(),
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
            .replacing(vec![self.from.clone(), upstream(&self.old)]);
        rules::validate_write(&candidate, &Scope::default())
    }

    /// The `SKILL.md` first (in the old folder), then `skiman.toml`,
    /// `skiman.lock`, the snapshot and, last, the folder.
    fn change(&self) -> Change {
        let (old, new) = (&self.old, &self.new);
        let mut pieces = Vec::new();
        if let Some(content) = &self.skill_md {
            let rel = format!("{}/SKILL.md", self.from);
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
        if self.has_upstream {
            let from = Want::From(format!("upstream/{old}"));
            pieces.push((format!("upstream/{new}"), from));
        }
        pieces.push((self.to.clone(), Want::From(self.from.clone())));
        Change {
            pieces,
            command: "mv",
            save_files: false,
        }
    }
}
