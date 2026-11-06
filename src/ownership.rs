//! Which tool-directory entries and instruction locations skiman owns, and
//! what every other entry is (spec Ownership). Link targets are compared
//! lexically; the target need not exist.

use std::ffi::OsStr;
use std::fs::Metadata;
use std::path::{Path, PathBuf};

use crate::fsutil::absolute_lexical;
use crate::repo::SkillRepo;
use crate::tools::{ManagedElsewhere, PLUGIN_MARKER, TOOLS};

/// What a tool-directory entry is, decided in the spec's order: ownership first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A link skiman made; holds its normalised target.
    Owned(PathBuf),
    /// Another manager owns it: never touched, never reported as foreign.
    ManagedElsewhere,
    /// A skill folder (or a link to one) that skiman does not own.
    Foreign,
    /// Not owned and not a skill folder: a regular file, a folder without
    /// `SKILL.md`, a name starting with `.`, a dangling link.
    NotASkill,
}

/// Classifies tool-directory entries for one skill repo.
#[derive(Debug, Clone)]
pub struct Ownership {
    skills_dir: PathBuf,
    managing_skills_dir: PathBuf,
    shared_body: PathBuf,
    /// Realpath of the skill repo `init` just switched away from: links under
    /// it are owned for that one sync.
    previous_repo: Option<PathBuf>,
    /// Managed-elsewhere paths, as realpath of the parent plus the file name.
    managed_paths: Vec<PathBuf>,
    /// Realpaths of the directories whose content is managed elsewhere.
    managed_under: Vec<PathBuf>,
}

impl Ownership {
    pub fn new(home: &Path, repo: &SkillRepo, previous_repo: Option<PathBuf>) -> Ownership {
        let mut managed_paths = Vec::new();
        let mut managed_under = Vec::new();
        for managed in TOOLS.iter().flat_map(|tool| tool.managed_elsewhere) {
            match *managed {
                ManagedElsewhere::Path(rel) => {
                    let path = home.join(rel);
                    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
                        && let Ok(parent) = parent.canonicalize()
                    {
                        managed_paths.push(parent.join(name));
                    }
                }
                ManagedElsewhere::RealpathUnder(rel) => {
                    if let Ok(dir) = home.join(rel).canonicalize() {
                        managed_under.push(dir);
                    }
                }
            }
        }
        // Skills under a project root's `skill_dirs` reach the tools through
        // links another manager keeps (plan "Configuration").
        managed_under.extend(crate::project::root::configured_skill_dirs(home, repo));
        Ownership {
            skills_dir: repo.skills_dir(),
            managing_skills_dir: repo.managing_skills_dir(),
            shared_body: repo.shared_body(),
            previous_repo,
            managed_paths,
            managed_under,
        }
    }

    /// Classifies the entry `name` in the tool directory whose realpath is
    /// `real_dir`; `meta` is the entry's own metadata (link not followed).
    pub fn classify(&self, real_dir: &Path, name: &OsStr, meta: &Metadata) -> Entry {
        let path = real_dir.join(name);
        if meta.is_symlink()
            && let Some(target) = link_target(real_dir, &path)
            && self.is_owned_target(name, &target)
        {
            return Entry::Owned(target);
        }
        if self.managed_paths.iter().any(|p| path.starts_with(p)) {
            return Entry::ManagedElsewhere;
        }
        let real = if meta.is_symlink() {
            match path.canonicalize() {
                Ok(real) => real,
                Err(_) => return Entry::NotASkill,
            }
        } else {
            path.clone()
        };
        if self.managed_under.iter().any(|dir| real.starts_with(dir)) {
            return Entry::ManagedElsewhere;
        }
        if name.as_encoded_bytes().starts_with(b".") || !real.is_dir() {
            return Entry::NotASkill;
        }
        if real.join(PLUGIN_MARKER).is_file() {
            return Entry::ManagedElsewhere;
        }
        if real.join("SKILL.md").is_file() {
            Entry::Foreign
        } else {
            Entry::NotASkill
        }
    }

    /// The normalised target of the instruction location `location` (the
    /// realpath of its parent joined with the file name) when skiman owns it:
    /// a symlink to the shared body, or under the previous skill repo.
    pub fn owned_instruction(&self, location: &Path, meta: &Metadata) -> Option<PathBuf> {
        if !meta.is_symlink() {
            return None;
        }
        let target = link_target(location.parent()?, location)?;
        (target == self.shared_body || self.is_under_previous_repo(&target)).then_some(target)
    }

    fn is_owned_target(&self, name: &OsStr, target: &Path) -> bool {
        *target == self.skills_dir.join(name)
            || *target == self.managing_skills_dir
            || self.is_under_previous_repo(target)
    }

    fn is_under_previous_repo(&self, target: &Path) -> bool {
        self.previous_repo
            .as_ref()
            .is_some_and(|previous| target.starts_with(previous))
    }
}

/// The link target of the symlink at `entry` (inside the directory whose
/// realpath is `real_dir`), made absolute and normalised lexically.
/// `None` when `entry` is not a symlink.
pub fn link_target(real_dir: &Path, entry: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(entry).ok()?;
    Some(absolute_lexical(real_dir, &target))
}
