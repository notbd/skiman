//! Skill names: the name rule, reserved names and taken names (spec Rules,
//! Agent tools, Source fetching). Shared by `new`, `add`, `mv`, `adopt`,
//! `import` and sync's distributability test.

use crate::failure::Failure;
use crate::lockfile::Lockfile;
use crate::manifest::Manifest;
use crate::repo::SkillRepo;
use crate::tools::TOOLS;

/// Reserved by skiman itself; each agent tool can reserve more (`AgentTool::reserved_names`).
pub const RESERVED: &[&str] = &["managing-skills"];

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

/// `managing-skills`, any agent tool's reserved name, or a name starting with `.`.
pub fn is_reserved(name: &str) -> bool {
    name.starts_with('.')
        || RESERVED.contains(&name)
        || TOOLS.iter().any(|tool| tool.reserved_names.contains(&name))
}

/// Refuses (exit 1) a name that is reserved or breaks the name rule.
pub fn check_new_name(name: &str) -> Result<(), Failure> {
    if is_reserved(name) {
        return Err(Failure::refused(format!(
            "`{name}` is a reserved skill name (reserved: managing-skills, synced, and any name starting with `.`)"
        )));
    }
    if !is_valid(name) {
        return Err(Failure::refused(format!(
            "`{name}` is not a valid skill name: use 1 to {MAX_LEN} lowercase letters, digits and single hyphens between them (^[a-z0-9]+(-[a-z0-9]+)*$)"
        )));
    }
    Ok(())
}

/// Refuses (exit 1) a **taken** name: a `skills/<name>` entry in any form, a
/// `skiman.toml` table, a lock entry or an upstream snapshot.
pub fn check_not_taken(repo: &SkillRepo, name: &str) -> Result<(), Failure> {
    let exists = |p: std::path::PathBuf| std::fs::symlink_metadata(p).is_ok();
    let reason = if exists(repo.skill_dir(name)) {
        Some(format!("skills/{name} already exists"))
    } else if Manifest::load(repo)?.skills().contains_key(name) {
        Some(format!("skiman.toml already has a [skills.{name}] table"))
    } else if Lockfile::load(repo)?.skill_names().contains(name) {
        Some(format!("skiman.lock already has an entry for {name}"))
    } else if exists(repo.upstream_dir(name)) {
        Some(format!("upstream/{name} already exists"))
    } else {
        None
    };
    match reason {
        Some(reason) => Err(Failure::refused(format!(
            "the name `{name}` is taken: {reason}"
        ))),
        None => Ok(()),
    }
}
