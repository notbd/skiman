//! The skill repo on disk: its layout (spec Data formats, "Skill repo layout")
//! and how commands find it through the user config.

use std::path::{Path, PathBuf};

use crate::config::UserConfig;
use crate::failure::Failure;
use crate::paths::Paths;

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

    /// `skills/<name>`: the canonical copy of a skill, and the target of its links.
    pub fn skill_dir(&self, name: &str) -> PathBuf {
        self.skills_dir().join(name)
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

    /// Names of the entries under `skills/` (any kind of entry), sorted.
    /// Names that are not valid UTF-8 are skipped: they can never be distributable.
    pub fn skill_folder_names(&self) -> std::io::Result<Vec<String>> {
        let entries = match std::fs::read_dir(self.skills_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut names = Vec::new();
        for entry in entries {
            if let Ok(name) = entry?.file_name().into_string() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }
}
