//! Which tool-directory entries and instruction locations skiman owns, and
//! what every other entry is (spec Ownership). Link targets are compared
//! lexically; the target need not exist.

use std::ffi::OsStr;
use std::fs::Metadata;
use std::path::{Path, PathBuf};

use crate::backup::noted_copy;
use crate::canon::Canon;
use crate::fsutil::absolute_lexical;
use crate::paths::Paths;
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
    /// What a move to a backup on another filesystem left when it was cut
    /// short: an entry whose note names a copy ([`noted_copy`], which reads
    /// no content), held here. Moving it again finishes the move.
    Moved(PathBuf),
}

/// Classifies tool-directory entries for one canon.
#[derive(Debug, Clone)]
pub struct Ownership {
    skills_dir: PathBuf,
    manual_dir: PathBuf,
    retired_manual_dir: PathBuf,
    shared_body: PathBuf,
    /// `<canon>/linked`: links to `<id>/<name>` or `<id>` in it are owned.
    linked_dir: PathBuf,
    /// Realpath of the canon `init` just switched away from: links under
    /// it are owned for that one sync.
    previous_repo: Option<PathBuf>,
    /// Managed-elsewhere paths, as realpath of the parent plus the file name.
    managed_paths: Vec<PathBuf>,
    /// Realpaths of the directories whose content is managed elsewhere.
    managed_under: Vec<PathBuf>,
    /// `<state dir>/backups`: where the copy a note names must be.
    backups: PathBuf,
}

impl Ownership {
    pub fn new(paths: &Paths, repo: &Canon, previous_repo: Option<PathBuf>) -> Ownership {
        let home = paths.home();
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
        Ownership {
            skills_dir: repo.skills_dir(),
            manual_dir: repo.manual_dir(),
            retired_manual_dir: repo.retired_manual_dir(),
            shared_body: repo.shared_body(),
            linked_dir: repo.linked_dir(),
            previous_repo,
            managed_paths,
            managed_under,
            backups: paths.state_dir().join("backups"),
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
        if let Some(copy) = noted_copy(&path, &self.backups) {
            return Entry::Moved(copy);
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
    /// a symlink to the shared body, or under the previous canon.
    pub fn owned_instruction(&self, location: &Path, meta: &Metadata) -> Option<PathBuf> {
        if !meta.is_symlink() {
            return None;
        }
        let target = link_target(location.parent()?, location)?;
        (target == self.shared_body || self.is_under_previous_repo(&target)).then_some(target)
    }

    /// A link to `skills/<name>`, or to `skills/<x>/<name>` (`<x>` one or
    /// more folders) when no folder of `<x>` is a skill folder (a group,
    /// whichever: a skill that changed group keeps its link until sync
    /// retargets it; a folder inside a skill is not one). A link under a
    /// temporary name is owned as its name: a sync killed while it removed
    /// or retargeted it left it there.
    fn is_owned_target(&self, name: &OsStr, target: &Path) -> bool {
        let name = crate::fsutil::temporary_of(name).unwrap_or(name);
        let in_skills = |dir: &Path| {
            dir.starts_with(&self.skills_dir)
                && dir
                    .ancestors()
                    .take_while(|d| *d != self.skills_dir)
                    .all(|d| std::fs::symlink_metadata(d.join("SKILL.md")).is_err())
        };
        // A linked skill: `linked/<id>/<name>`, or `linked/<id>` for a
        // folder that is one skill (ADR 0010).
        let in_linked =
            |dir: &Path| dir == self.linked_dir || dir.parent() == Some(self.linked_dir.as_path());
        (target.file_name() == Some(name)
            && target
                .parent()
                .is_some_and(|d| in_skills(d) || in_linked(d)))
            || *target == self.manual_dir
            || *target == self.retired_manual_dir
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    /// A canon and a tool directory holding a link `name` to `target`
    /// (relative to the canon): what the link is.
    fn classify(name: &str, target: &str) -> Entry {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (home, canon) = (root.join("home"), root.join("canon"));
        let tool_dir = home.join(".claude/skills");
        std::fs::create_dir_all(&tool_dir).unwrap();
        std::fs::create_dir_all(&canon).unwrap();
        symlink(canon.join(target), tool_dir.join(name)).unwrap();
        let repo = Canon::at(&canon).unwrap();
        let ownership = Ownership::new(&Paths::under(&home), &repo, None);
        let meta = std::fs::symlink_metadata(tool_dir.join(name)).unwrap();
        ownership.classify(&tool_dir, OsStr::new(name), &meta)
    }

    fn owned(entry: &Entry) -> bool {
        matches!(entry, Entry::Owned(_))
    }

    /// Both shapes of a linked skill's link are skiman's whether or not
    /// `linked/<id>` exists, so a deleted registration's links are still found.
    #[test]
    fn a_link_to_a_linked_skill_is_owned_in_both_shapes() {
        assert!(owned(&classify("pdf", "linked/company/pdf")));
        assert!(owned(&classify("solo", "linked/solo")));
    }

    #[test]
    fn a_link_under_a_temporary_name_of_a_linked_skill_is_owned() {
        assert!(owned(&classify(".pdf.skiman-tmp-42", "linked/company/pdf")));
        assert!(owned(&classify(".solo.skiman-tmp-42-1", "linked/solo")));
    }

    #[test]
    fn no_other_target_under_linked_is_owned() {
        for (name, target) in [
            ("pdf", "linked/company/docx"),
            ("solo", "linked/other"),
            ("pdf", "linked/company/sub/pdf"),
            ("linked", "linked"),
            ("company", "linked/company/company/x"),
        ] {
            assert!(!owned(&classify(name, target)), "{name} -> {target}");
        }
    }
}
