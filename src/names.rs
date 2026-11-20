//! Skill names: the name rule, reserved names and taken names (spec Rules,
//! Agent tools, Source fetching; a linked skill's name is taken too).
//! Shared by `new`, `add`, `mv`, `adopt`, `import` and sync's
//! distributability test.

use crate::canon::Canon;
use crate::failure::Failure;
use crate::linked::{Holder, Linked};
use crate::lockfile::Lockfile;
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::sync;
use crate::tools::TOOLS;

/// Reserved by skiman itself; each agent tool can reserve more (`AgentTool::reserved_names`).
pub const RESERVED: &[&str] = &[crate::skiman_manual::NAME];

pub const MAX_LEN: usize = 64;

/// `^[a-z0-9]+(-[a-z0-9]+)*$` and 1 to 64 characters.
pub fn is_valid(name: &str) -> bool {
    (1..=MAX_LEN).contains(&name.len())
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// `skiman-manual`, any agent tool's reserved name, or a name starting with `.`.
pub fn is_reserved(name: &str) -> bool {
    name.starts_with('.')
        || RESERVED.contains(&name)
        || TOOLS.iter().any(|tool| tool.reserved_names.contains(&name))
}

/// Refuses (exit 1) a name that is reserved or breaks the name rule.
pub fn check_new_name(name: &str) -> Result<(), Failure> {
    if is_reserved(name) {
        return Err(Failure::refused(format!(
            "`{name}` is a reserved skill name (reserved: skiman-manual, synced, and any name starting with `.`)"
        )));
    }
    if !is_valid(name) {
        return Err(Failure::refused(format!(
            "`{name}` is not a valid skill name: use 1 to {MAX_LEN} lowercase letters, digits and single hyphens between them (^[a-z0-9]+(-[a-z0-9]+)*$)"
        )));
    }
    Ok(())
}

/// Refuses (exit 1) a group (`a` or `a/b`) with a folder name that breaks
/// the name rule, or that names the skill `name` going into it, a skill
/// folder in the canon or a linked skill's name ([`linked_name`]; a group
/// folder and a skill never share a name).
pub fn check_group(paths: &Paths, repo: &Canon, group: &str, name: &str) -> Result<(), Failure> {
    let layout = repo.layout()?;
    for folder in group.split('/') {
        if !is_valid(folder) {
            return Err(Failure::refused(format!(
                "`{group}` is not a valid group: give one or more group folder names joined with `/`, each 1 to {MAX_LEN} lowercase letters, digits and single hyphens between them (^[a-z0-9]+(-[a-z0-9]+)*$)"
            )));
        }
        if folder == name {
            return Err(Failure::refused(format!(
                "`{group}` cannot be the group of a skill named `{name}`: a group folder and a skill never share a name"
            )));
        }
        if let Some(skill) = layout.find(folder) {
            return Err(Failure::refused(format!(
                "`{group}` cannot be a group: {} is a skill folder named `{folder}`",
                skill.path()
            )));
        }
        match linked_name(paths, repo, folder)? {
            Some(LinkedName::Skill(it)) => {
                return Err(Failure::refused(format!(
                    "`{group}` cannot be a group: `{folder}` is {it}"
                )));
            }
            Some(LinkedName::Kept(holder)) => {
                return Err(Failure::refused(format!(
                    "`{group}` cannot be a group: {holder} `{folder}`"
                )));
            }
            None => {}
        }
    }
    Ok(())
}

/// What can hold a skill name in the canon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    Folder,
    Table,
    LockEntry,
    Snapshot,
}

/// Refuses (exit 1) a **taken** name: a skill folder entry in any form in
/// any group, a group folder, a `skiman.toml` table, a lock entry or an
/// upstream snapshot, except one that `ours` says is what this command
/// writes, left by a run of it that stopped part-way (which then finishes);
/// and a linked skill's name, always (spec "Names and collisions";
/// [`linked_name`]).
pub fn check_not_taken(
    paths: &Paths,
    repo: &Canon,
    name: &str,
    ours: impl Fn(Taken) -> Result<bool, Failure>,
) -> Result<(), Failure> {
    let exists = |p: std::path::PathBuf| std::fs::symlink_metadata(p).is_ok();
    let layout = repo.layout()?;
    let reason = if let Some(placed) = layout.find(name)
        && !ours(Taken::Folder)?
    {
        Some(format!("{} already exists", placed.path()))
    } else if let Some(group) = layout.group_named(name) {
        Some(format!("skills/{group} is a group folder"))
    } else if Manifest::load(repo)?.skills().contains_key(name) && !ours(Taken::Table)? {
        Some(format!("skiman.toml already has a [skills.{name}] table"))
    } else if Lockfile::load(repo)?.skill_names().contains(name) && !ours(Taken::LockEntry)? {
        Some(format!("skiman.lock already has an entry for {name}"))
    } else if exists(repo.upstream_dir(name)) && !ours(Taken::Snapshot)? {
        Some(format!("upstream/{name} already exists"))
    } else {
        None
    };
    match reason {
        Some(reason) => Err(taken(name, &reason)),
        None => check_not_linked(paths, repo, name),
    }
}

/// Refuses (exit 1) a linked skill's name, as [`check_not_taken`] does:
/// for a command that finishes a run that stopped part-way, whose own
/// records may hold the name.
pub fn check_not_linked(paths: &Paths, repo: &Canon, name: &str) -> Result<(), Failure> {
    match linked_name(paths, repo, name)? {
        Some(LinkedName::Skill(it)) => Err(taken(name, &format!("it is {it}"))),
        Some(LinkedName::Kept(holder)) => Err(taken(name, &holder.to_string())),
        None => Ok(()),
    }
}

/// What holds a name among the linked skills.
enum LinkedName {
    /// A linked skill, as [`Linked::described`] says it.
    Skill(String),
    /// A link sync keeps while its folder or `SKILL.md` cannot be read
    /// ([`sync::kept_names`]): the name stays taken, so the canon never
    /// takes it over once the folder can be read again.
    Kept(Holder),
}

fn linked_name(paths: &Paths, repo: &Canon, name: &str) -> Result<Option<LinkedName>, Failure> {
    let manifest = Manifest::load(repo)?;
    let linked = Linked::read(paths.home(), repo, &manifest);
    let mut kept = sync::kept_names(paths, repo, &manifest, &linked);
    if let Some((it, _)) = linked.described(name) {
        return Ok(Some(LinkedName::Skill(it)));
    }
    Ok(kept.remove(name).map(LinkedName::Kept))
}

/// Why a command that acts on a skill in the canon refuses `name`, which
/// is a linked skill ([`Linked::edited_elsewhere`] with the names sync
/// keeps), and the linked folder's id. Call it when nothing in the canon
/// has the name.
pub fn edited_elsewhere(
    paths: &Paths,
    repo: &Canon,
    manifest: &Manifest,
    name: &str,
) -> Option<(String, String)> {
    let linked = Linked::read(paths.home(), repo, manifest);
    let kept = sync::kept_names(paths, repo, manifest, &linked);
    let (why, id) = linked.edited_elsewhere(name, &kept)?;
    Some((why, id.to_owned()))
}

fn taken(name: &str, reason: &str) -> Failure {
    Failure::refused(format!("the name `{name}` is taken: {reason}"))
}
