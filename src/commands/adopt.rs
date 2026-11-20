//! `skiman adopt <name> --from <tool id> [--group <group>]`: bring a foreign
//! skill in as an own skill, in a group folder when one is given. Reads the entry's resolved content (path safety applies, `.git/`
//! is not skill content), rewrites the `name` line when needed, locates and
//! reads the entry again under the canon lock (refusing when the copy
//! changed), checks the whole copy as new content, stages it under
//! `.skiman/tmp/`, renames it to `skills/[<group>/]<name>`, moves the original entry
//! (a link as a link) to `backups/<timestamp>/<tool id>/<name>`, then syncs,
//! which links the copy in its place. A link's target is never moved,
//! edited or deleted. An adopt stopped before the original moved finishes
//! when it is run again: a `skills/<name>` that holds the copy is kept, and
//! so is a cut-short move of the original to the backup.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup::{self, Backups};
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::change::{self, Change, Want};
use crate::commands::{confirm, has_terminal};
use crate::failure::{Failure, finish};
use crate::layout::skill_path;
use crate::linked::{Linked, State};
use crate::names::{self, Taken};
use crate::ownership::{Entry, Ownership};
use crate::paths::Paths;
use crate::rules::{self, Scope};
use crate::skill_md::SkillMd;
use crate::snapshot::{Snapshot, SnapshotFile};
use crate::sync::{self, Output};
use crate::test_hook;
use crate::tools::{AgentTool, TOOLS};
use crate::tree::WorkingTree;

pub fn run(name: &str, from: &str, yes: bool, group: Option<&str>) -> Exit {
    finish(adopt(name, from, yes, group))
}

fn adopt(name: &str, from: &str, yes: bool, group: Option<&str>) -> Result<Exit, Failure> {
    if !yes && !has_terminal() {
        return Err(Failure::usage("missing flag without a terminal: --yes"));
    }
    let paths = Paths::from_env()?;
    let repo = Canon::from_config(&paths)?;
    let tool = TOOLS
        .iter()
        .find(|tool| tool.id == from)
        .expect("clap accepts only known tool ids");
    let entry = tool.tool_dir(paths.home()).join(name);
    check_name(&paths, &repo, name, &entry, None)?;
    if let Some(group) = group {
        names::check_group(&paths, &repo, group, name)?;
    }
    let (entry, files) = read(&paths, &repo, tool, name)?;
    let change = Change {
        pieces: vec![(skill_path(group, name), Want::Folder(files.clone()))],
        command: "adopt",
        save_files: false,
    };
    check_name(&paths, &repo, name, &entry, Some(&change))?;
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
    let lock = CanonLock::wait(&paths, &repo)?;
    check_name(&paths, &repo, name, &entry, Some(&change))?;
    if let Some(group) = group {
        names::check_group(&paths, &repo, group, name)?;
    }
    // The entry may have moved, come under another manager or changed while
    // adopt waited. The copies compare as written (`name` line rewritten,
    // `.git/` left out), so only a change to what would be adopted refuses.
    if read(&paths, &repo, tool, name)? != (entry.clone(), files) {
        return Err(Failure::refused(format!(
            "{} changed after adopt read it; nothing was changed, run adopt again",
            entry.display()
        )));
    }
    validate(&repo, name, &change)?;
    let mut backups = Backups::new(&paths);
    let nothing = |e| Failure::operational(format!("{e}; nothing was changed"));
    backups.dir().map_err(nothing)?;
    change.apply(&repo, &lock, &mut backups)?;
    test_hook::at("adopt", "placed");
    let moved = backups
        .move_in(&entry, &Path::new(tool.id).join(name))
        .map_err(|e| {
            change::stopped(Failure::operational(format!(
                "cannot move {} to the backup: {e}; {} is in place",
                entry.display(),
                skill_path(group, name)
            )))
        })?;
    out!("moved {} to {}", entry.display(), moved.display());
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}

/// Refuses (exit 1) a name that fails the name rules, is reserved or is
/// taken in the canon, telling the user to rename or move the entry.
/// With the `change` adopt writes, a `skills/<name>` that already holds the
/// copy does not take the name.
fn check_name(
    paths: &Paths,
    repo: &Canon,
    name: &str,
    entry: &Path,
    change: Option<&Change>,
) -> Result<(), Failure> {
    names::check_new_name(name)
        .and_then(|()| match change {
            None => Ok(()),
            Some(c) => names::check_not_taken(paths, repo, name, |taken| {
                Ok(taken == Taken::Folder && c.in_place(repo.root())?)
            }),
        })
        .map_err(|f| {
            Failure::refused(format!(
                "{}; rename or move the foreign entry {} by hand (to adopt it, give it a valid, free name and adopt that name)",
                f.message,
                entry.display()
            ))
        })
}

/// The path of the foreign entry `name` in `tool`'s tool directory (inside
/// the directory's realpath) and the files of its copy ([`read_copy`]).
/// Refuses (exit 1) anything but a foreign skill, or, once `skills/<name>`
/// is in place (an adopt stopped part-way), what a move of the entry to the
/// backup left when it was cut short, which is read from its complete copy.
/// Nothing is copied out of a linked folder (spec "Nothing is moved out of
/// a linked folder"): refuses an entry that sits in one or resolves into
/// one, and any entry while a linked folder cannot be read.
fn read(
    paths: &Paths,
    repo: &Canon,
    tool: &AgentTool,
    name: &str,
) -> Result<(PathBuf, Vec<SnapshotFile>), Failure> {
    let tool_dir = tool.tool_dir(paths.home());
    // An adopt that stopped part-way may have moved the entry already.
    let skill_dir = repo.skill_dir(name)?;
    let done = match skill_dir.is_dir() {
        true => format!(
            "; if `skiman adopt {name}` stopped part-way, {} is in place and `skiman sync` finishes it",
            skill_dir
                .strip_prefix(repo.root())
                .unwrap_or(&skill_dir)
                .display()
        ),
        false => String::new(),
    };
    let missing = || Failure::refused(format!("{} has no entry {name}{done}", tool_dir.display()));
    let linked = Linked::load(paths.home(), repo)?;
    if let Some(folder) = linked.unreadable() {
        return Err(Failure::refused(format!(
            "linked folder `{}` cannot be read now, so skiman cannot tell whether {} leads into it; nothing was changed, run `skiman sync` to see why",
            folder.id,
            tool_dir.join(name).display()
        )));
    }
    let real_dir = tool_dir.canonicalize().map_err(|_| missing())?;
    let entry = real_dir.join(name);
    let meta = std::fs::symlink_metadata(&entry).map_err(|_| missing())?;
    // Where the entry sits: moving it to the backup would move it out of the folder.
    if let Some(folder) = linked.reaches_into(&entry) {
        return Err(Failure::refused(format!(
            "{} lies in linked folder `{}`, which skiman never moves anything out of; nothing was changed",
            entry.display(),
            folder.id
        )));
    }
    let ownership = Ownership::new(paths, repo, None);
    let refused = match ownership.classify(&real_dir, entry.file_name().expect("named"), &meta) {
        Entry::Foreign => {
            if let Some(folder) = entry
                .canonicalize()
                .ok()
                .and_then(|r| linked.reaches_into(&r))
            {
                // As sync says it: only a folder that was read is distributed.
                let hint = match folder.state {
                    State::Read(_) => {
                        "; skiman distributes the folder's skills already, so delete the link once nothing needs it"
                    }
                    _ => "",
                };
                return Err(Failure::refused(format!(
                    "{} resolves into linked folder `{}`, whose content skiman never copies into the canon{hint}",
                    entry.display(),
                    folder.id
                )));
            }
            let files = read_copy(&entry, name)?;
            return Ok((entry, files));
        }
        Entry::Moved(copy) if skill_dir.exists() => {
            let files = read_copy(&copy, name)?;
            return Ok((entry, files));
        }
        Entry::Owned(_) => format!(
            "{} is skiman's own link; there is nothing to adopt{done}",
            entry.display()
        ),
        Entry::ManagedElsewhere => format!(
            "{} is managed by another tool; skiman leaves it alone",
            entry.display()
        ),
        Entry::Moved(copy) => {
            backup::cut_short(&entry, &copy)
                + "; to adopt the skill, delete it and move that copy here first"
        }
        Entry::NotASkill => format!(
            "{} is not a skill folder (a folder, or a link to one, holding SKILL.md)",
            entry.display()
        ),
    };
    Err(Failure::refused(refused))
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
fn validate(repo: &Canon, name: &str, change: &Change) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(repo.root());
    let scope = Scope {
        skills: BTreeSet::from([name.to_owned()]),
        consistency: BTreeSet::from([name.to_owned()]),
        ..Scope::default()
    };
    rules::validate_write(&change.overlay(&working_tree), &scope)
}
