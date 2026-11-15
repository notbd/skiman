//! The skill repo on disk: its layout (spec Data formats, "Skill repo layout")
//! and how commands find it through the user config.

use std::path::{Path, PathBuf};

use crate::config::UserConfig;
use crate::failure::Failure;
use crate::layout::Layout;
use crate::paths::Paths;
use crate::tree::WorkingTree;

/// A skill repo, addressed by its realpath (ownership compares link targets
/// against the realpath, spec Ownership).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRepo {
    root: PathBuf,
}

impl SkillRepo {
    /// The skill repo at `path`, which must exist.
    pub fn at(path: &Path) -> Result<SkillRepo, Failure> {
        let root = path.canonicalize().map_err(|e| {
            Failure::operational(format!(
                "skill repo {} is not reachable: {e}",
                path.display()
            ))
        })?;
        Ok(SkillRepo { root })
    }

    /// The skill repo the user config points to.
    pub fn from_config(paths: &Paths) -> Result<SkillRepo, Failure> {
        SkillRepo::at(&UserConfig::load(paths)?.repo)
    }

    /// Realpath of the skill repo.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    /// What `skills/` holds now: where each skill sits, and the group folders.
    pub fn layout(&self) -> Result<Layout, Failure> {
        Layout::read(&WorkingTree::new(&self.root)).map_err(|e| {
            Failure::operational(format!("cannot read {}: {e}", self.skills_dir().display()))
        })
    }

    /// `skills/<name>` or `skills/<group>/<name>`: the canonical copy of a
    /// skill, and the target of its links; `skills/<name>` when there is none.
    pub fn skill_dir(&self, name: &str) -> Result<PathBuf, Failure> {
        Ok(self.root.join(self.layout()?.path(name)))
    }

    /// Removes the group folders of the skill folder `skill` (relative to
    /// the skill repo) that nothing is left in, innermost first, printing a
    /// line for each; a skill in no group has none.
    pub fn remove_empty_group(&self, skill: &str) {
        let groups = Path::new(skill)
            .ancestors()
            .skip(1)
            .take_while(|g| *g != Path::new("skills") && *g != Path::new(""));
        for group in groups {
            let group = self.root.join(group);
            if std::fs::remove_dir(&group).is_err() {
                return;
            }
            out!("removed the empty group folder {}", group.display());
        }
    }

    /// `upstream/<name>`: the upstream snapshot of a third-party skill.
    pub fn upstream_dir(&self, name: &str) -> PathBuf {
        self.root.join("upstream").join(name)
    }

    /// `instructions/shared.md`: the shared body.
    pub fn shared_body(&self) -> PathBuf {
        self.root.join("instructions/shared.md")
    }

    pub fn manifest_file(&self) -> PathBuf {
        self.root.join("skiman.toml")
    }

    pub fn lockfile(&self) -> PathBuf {
        self.root.join("skiman.lock")
    }

    /// `.skiman/`: git-ignored, machine-local.
    pub fn local_dir(&self) -> PathBuf {
        self.root.join(".skiman")
    }

    /// `.skiman/lock`: the repo lock file.
    pub fn repo_lock_file(&self) -> PathBuf {
        self.local_dir().join("lock")
    }

    /// `.skiman/tmp/`: staging for write commands (`crate::staging`).
    pub fn tmp_dir(&self) -> PathBuf {
        self.local_dir().join("tmp")
    }

    /// `.skiman/import/`: the default import folder.
    pub fn default_import_dir(&self) -> PathBuf {
        self.local_dir().join("import")
    }

    /// `.skiman/managing-skills`: the materialised `managing-skills` skill.
    pub fn managing_skills_dir(&self) -> PathBuf {
        self.local_dir().join("managing-skills")
    }
}
