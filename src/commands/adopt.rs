//! `skiman adopt <name> --from <tool id>`: bring a foreign skill in as an own
//! skill. Reads the entry's resolved content (path safety applies, `.git/`
//! is not skill content), rewrites the `name` line when needed, locates and
//! reads the entry again under the repo lock (refusing when the copy
//! changed), checks the whole copy as new content, stages it under
//! `.skiman/tmp/`, renames it to `skills/<name>`, moves the original entry
//! (a link as a link) to `backups/<timestamp>/<tool id>/<name>`, then syncs,
//! which links the copy in its place. A link's target is never moved,
//! edited or deleted.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup::Backups;
use crate::commands::{confirm, has_terminal};
use crate::failure::{Failure, finish};
use crate::fsutil;
use crate::manifest::Manifest;
use crate::names;
use crate::ownership::{Entry, Ownership};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::skill_md::SkillMd;
use crate::snapshot::{Snapshot, SnapshotFile};
use crate::staging::{self, Staging};
use crate::sync::{self, Output};
use crate::test_hook;
use crate::tools::{AgentTool, TOOLS};
use crate::tree::{Overlay, WorkingTree};

pub fn run(name: &str, from: &str, yes: bool) -> Exit {
    finish(adopt(name, from, yes))
}

fn adopt(name: &str, from: &str, yes: bool) -> Result<Exit, Failure> {
    if !yes && !has_terminal() {
        return Err(Failure::usage("missing flag without a terminal: --yes"));
    }
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let tool = TOOLS
        .iter()
        .find(|tool| tool.id == from)
        .expect("clap accepts only known tool ids");
    let entry = tool.tool_dir(paths.home()).join(name);
    check_name(&repo, name, &entry)?;
    let manifest = Manifest::load(&repo)?;
    let entry = locate(&paths, &repo, &manifest, tool, name)?;
    let files = read_copy(&entry, name)?;
    if !yes {
        out!("skill {name} from {}:", entry.display());
        for file in &files {
            out!("  {}", file.path);
        }
        if !confirm(&format!("Adopt skill {name}? [y/N] "))? {
            return Err(Failure::refused("not adopted; nothing was changed"));
        }
    }

    test_hook::at("adopt", "confirmed");
    let lock = RepoLock::wait(&repo)?;
    check_name(&repo, name, &entry)?;
    // The entry may have moved, come under another manager or changed while
    // adopt waited. The copies compare as written (`name` line rewritten,
    // `.git/` left out), so only a change to what would be adopted refuses.
    let manifest = Manifest::load(&repo)?;
    if locate(&paths, &repo, &manifest, tool, name)? != entry || read_copy(&entry, name)? != files {
        return Err(Failure::refused(format!(
            "{} changed after adopt read it; nothing was changed, run adopt again",
            entry.display()
        )));
    }
    validate(&repo, name, &files)?;
    let staging = Staging::begin(&paths, &repo, &lock)?;
    let staged = stage(&staging, name, &files)?;
    let mut backups = Backups::new(&paths);
    let nothing = |e| Failure::operational(format!("{e}; nothing was changed"));
    let dir = backups.dir().map_err(nothing)?;
    let copy = repo.skill_dir(name);
    staging::rename_into_place(&staged, &copy)?;
    out!("created {}", copy.display());
    let rel = Path::new(tool.id).join(name);
    let moved = backups.move_in(&entry, &rel).map_err(|e| {
        Failure::operational(format!(
            "cannot move {} to the backup: {e}; skills/{name} is in place, so finish by hand: move {} to {}, then run `skiman sync`",
            entry.display(),
            entry.display(),
            dir.join(&rel).display()
        ))
    })?;
    out!("moved {} to {}", entry.display(), moved.display());
    drop(staging);
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}

/// Refuses (exit 1) a name that fails the name rules, is reserved or is
/// taken in the skill repo, telling the user to rename or move the entry.
fn check_name(repo: &SkillRepo, name: &str, entry: &Path) -> Result<(), Failure> {
    names::check_new_name(name)
        .and_then(|()| names::check_not_taken(repo, name))
        .map_err(|f| {
            Failure::refused(format!(
                "{}; rename or move the foreign entry {} by hand (to adopt it, give it a valid, free name and adopt that name)",
                f.message,
                entry.display()
            ))
        })
}

/// The path of the foreign entry `name` in `tool`'s tool directory (inside
/// the directory's realpath). Refuses (exit 1) anything but a foreign skill.
fn locate(
    paths: &Paths,
    repo: &SkillRepo,
    manifest: &Manifest,
    tool: &AgentTool,
    name: &str,
) -> Result<PathBuf, Failure> {
    let tool_dir = tool.tool_dir(paths.home());
    let missing = || Failure::refused(format!("{} has no entry {name}", tool_dir.display()));
    let real_dir = tool_dir.canonicalize().map_err(|_| missing())?;
    let entry = real_dir.join(name);
    let meta = std::fs::symlink_metadata(&entry).map_err(|_| missing())?;
    let ownership = Ownership::new(paths.home(), repo, manifest, None);
    match ownership.classify(&real_dir, entry.file_name().expect("named"), &meta) {
        Entry::Foreign => Ok(entry),
        Entry::Owned(_) => Err(Failure::refused(format!(
            "{} is skiman's own link; there is nothing to adopt",
            entry.display()
        ))),
        Entry::ManagedElsewhere => Err(Failure::refused(format!(
            "{} is managed by another tool; skiman leaves it alone",
            entry.display()
        ))),
        Entry::NotASkill => Err(Failure::refused(format!(
            "{} is not a skill folder (a folder, or a link to one, holding SKILL.md)",
            entry.display()
        ))),
    }
}

/// The files of the copy: the entry's resolved content as a local folder
/// (path safety applies; `.git/` and the ignored names in
/// [`crate::tree::IGNORED_NAMES`] are left out), with the `name` line of
/// `SKILL.md` rewritten when it differs.
fn read_copy(entry: &Path, name: &str) -> Result<Vec<SnapshotFile>, Failure> {
    let resolved = entry
        .canonicalize()
        .map_err(|e| Failure::operational(format!("cannot resolve {}: {e}", entry.display())))?;
    let mut snapshot = Snapshot::read_local(&resolved)?;
    let label = format!("{}/SKILL.md", entry.display());
    let Some(file) = snapshot.files.iter_mut().find(|f| f.path == "SKILL.md") else {
        return Err(Failure::refused(format!("{label} is not a regular file")));
    };
    let skill_md = SkillMd::parse(&label, &file.content)?;
    if skill_md.name() != Some(name) {
        file.content = skill_md.with_name(name)?.into_bytes();
    }
    Ok(snapshot.files)
}

/// The whole copy is new content.
fn validate(repo: &SkillRepo, name: &str, files: &[SnapshotFile]) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(repo.root());
    let path = |file: &SnapshotFile| format!("skills/{name}/{}", file.path);
    let executable = files.iter().filter(|f| f.executable).map(path).collect();
    let contents = files.iter().map(|f| (path(f), f.content.clone())).collect();
    let candidate = Overlay::new(&working_tree, contents).with_executable(executable);
    let scope = Scope {
        skills: BTreeSet::from([name.to_owned()]),
        consistency: BTreeSet::from([name.to_owned()]),
        ..Scope::default()
    };
    rules::validate_write(&candidate, &scope)
}

/// Writes the copy to `tmp/<timestamp>/skills/<name>` and returns that path.
fn stage(staging: &Staging, name: &str, files: &[SnapshotFile]) -> Result<PathBuf, Failure> {
    let dir = staging.dir().join("skills").join(name);
    fsutil::write_files(&dir, files)?;
    Ok(dir)
}
