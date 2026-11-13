//! `skiman mv <old> <new>`: rename a skill. Renames `skills/<old>`, its
//! `SKILL.md` `name` line, `upstream/<old>`, the `[skills.<old>]` table of
//! `skiman.toml` (with `enabled` and `source`), the lock entry and the
//! `[[secrets.allow]]` paths, then syncs, which links the new name and
//! removes the old links. Nothing is backed up: every piece is renamed in
//! place.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::commands::remove::check_single_name;
use crate::failure::{Failure, finish};
use crate::fsutil::{self, read_text, rewrite_atomic, write_atomic};
use crate::lockfile;
use crate::manifest;
use crate::names;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::skill_md::SkillMd;
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

    let lock = RepoLock::wait(&repo)?;
    if !std::fs::symlink_metadata(repo.skill_dir(old)).is_ok_and(|m| m.is_dir()) {
        return Err(Failure::refused(format!(
            "skills/{old} is not a skill folder"
        )));
    }
    names::check_not_taken(&repo, new)?;
    let plan = Plan::build(&repo, old, new)?;
    plan.validate(&repo)?;
    plan.apply(&repo)?;
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
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
            has_upstream: std::fs::symlink_metadata(repo.upstream_dir(old)).is_ok(),
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
            let from = format!("{root}/{}", self.old);
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

    /// Folder first, then its `SKILL.md`, the snapshot, `skiman.toml` and
    /// last `skiman.lock`. Each completed step is printed.
    fn apply(&self, repo: &SkillRepo) -> Result<(), Failure> {
        rename(&repo.skill_dir(&self.old), &repo.skill_dir(&self.new))?;
        if let Some(content) = &self.skill_md {
            let file = repo.skill_dir(&self.new).join("SKILL.md");
            let write = std::fs::metadata(&file)
                .and_then(|meta| rewrite_atomic(&file, content, meta.permissions()));
            write.map_err(|e| {
                Failure::operational(format!("cannot write {}: {e}", file.display()))
            })?;
            out!("changed {}", file.display());
        }
        if self.has_upstream {
            rename(&repo.upstream_dir(&self.old), &repo.upstream_dir(&self.new))?;
        }
        for (path, text) in [
            (repo.manifest_file(), &self.manifest),
            (repo.lockfile(), &self.lock),
        ] {
            if let Some(text) = text {
                write_atomic(&path, text.as_bytes()).map_err(|e| {
                    Failure::operational(format!("cannot write {}: {e}", path.display()))
                })?;
                out!("changed {}", path.display());
            }
        }
        Ok(())
    }
}

/// Renames `from` to `to`, which must not exist.
fn rename(from: &Path, to: &Path) -> Result<(), Failure> {
    let fail = |e: String| {
        Failure::operational(format!(
            "cannot rename {} to {}: {e}",
            from.display(),
            to.display()
        ))
    };
    fsutil::publish(from, to).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => {
            fail("it appeared while skiman was renaming".to_owned())
        }
        std::io::ErrorKind::CrossesDevices => Failure::operational(fsutil::crossing(from, to)),
        _ => fail(e.to_string()),
    })?;
    out!("renamed {} to {}", from.display(), to.display());
    Ok(())
}
