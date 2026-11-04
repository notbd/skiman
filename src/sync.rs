//! Distribution: the reconcile loop behind `skiman sync`, `sync --hook` and
//! the sync every write command ends with (spec Distribution and refresh).
//!
//! [`run`] is the entry point; it needs the repo lock. [`reconcile`] computes
//! and applies the changes and returns a [`Report`] of what it did and found.
//! Never spawns `git`: `sync --hook` must stay fast.

use std::path::{Path, PathBuf};

use crate::Exit;
use crate::failure::Failure;
use crate::managing_skills;
use crate::manifest::Manifest;
use crate::names;
use crate::ownership;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::tools::{AgentTool, TOOLS};

/// Who reads sync's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// A command run by the user: changes on stdout, problems on stderr.
    Normal,
    /// `sync --hook`: nothing on stdout unless there is something new to report; exit 0.
    Hook,
}

/// One entry sync wants in every installed tool directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub name: String,
    /// The exact absolute link target: `<repo realpath>/skills/<n>` or
    /// `<repo realpath>/.skiman/managing-skills`.
    pub target: PathBuf,
}

/// What one reconcile did and found.
#[derive(Debug, Default)]
pub struct Report {
    /// Links created, as tool-directory entry paths.
    pub linked: Vec<PathBuf>,
    /// Entries sync wanted to create whose path is taken by something it does
    /// not own. Left alone; reporting them is #5.
    pub occupied: Vec<PathBuf>,
    /// I/O failures (exit 4); sync goes on with the other entries.
    pub errors: Vec<String>,
}

impl Report {
    pub fn exit(&self) -> Exit {
        if self.errors.is_empty() {
            Exit::Success
        } else {
            Exit::OperationalError
        }
    }
}

/// Syncs `repo` while the caller holds its lock and prints the result for `output`.
pub fn run(paths: &Paths, repo: &SkillRepo, _lock: &RepoLock, output: Output) -> Exit {
    let report = match Manifest::load(repo) {
        Ok(manifest) => reconcile(paths.home(), repo, &manifest),
        Err(failure) => return report_failure(failure, output),
    };
    if output == Output::Normal {
        for link in &report.linked {
            println!("linked {}", link.display());
        }
    }
    for error in &report.errors {
        eprintln!("error: {error}");
    }
    match output {
        Output::Normal => report.exit(),
        Output::Hook => Exit::Success,
    }
}

/// Prints a failure that stops sync before it reconciles anything.
pub fn report_failure(failure: Failure, output: Output) -> Exit {
    let exit = failure.report();
    match output {
        Output::Normal => exit,
        Output::Hook => Exit::Success,
    }
}

/// Materialises `managing-skills`, then makes every installed tool's tool
/// directory hold an owned link for every wanted entry.
pub fn reconcile(home: &Path, repo: &SkillRepo, manifest: &Manifest) -> Report {
    let mut report = Report::default();
    if let Err(e) = managing_skills::materialise(repo) {
        report.errors.push(format!(
            "cannot write {}: {e}",
            repo.managing_skills_dir().display()
        ));
    }
    let wanted = match wanted_entries(repo, manifest) {
        Ok(wanted) => wanted,
        Err(e) => {
            report
                .errors
                .push(format!("cannot read {}: {e}", repo.skills_dir().display()));
            return report;
        }
    };
    for tool in installed_tools(home, manifest) {
        reconcile_tool_dir(&tool.tool_dir(home), &wanted, &mut report);
    }
    report
}

/// Tools that are enabled in `[tools].enabled` and whose presence directory exists.
pub fn installed_tools<'a>(
    home: &'a Path,
    manifest: &Manifest,
) -> impl Iterator<Item = &'static AgentTool> + 'a {
    let enabled = manifest.enabled_tools();
    TOOLS
        .iter()
        .filter(move |tool| enabled.iter().any(|id| id == tool.id) && tool.is_present(home))
}

/// Every distributable skill (valid, not reserved, folder holds `SKILL.md`,
/// not `enabled = false`) plus `managing-skills`.
pub fn wanted_entries(repo: &SkillRepo, manifest: &Manifest) -> std::io::Result<Vec<Wanted>> {
    let entries = manifest.skills();
    let mut wanted = Vec::new();
    for name in repo.skill_folder_names()? {
        let disabled = entries.get(&name).is_some_and(|e| !e.enabled);
        if !disabled && is_distributable(repo, &name) {
            wanted.push(Wanted {
                target: repo.skill_dir(&name),
                name,
            });
        }
    }
    wanted.push(Wanted {
        name: managing_skills::NAME.to_owned(),
        target: repo.managing_skills_dir(),
    });
    Ok(wanted)
}

/// The name passes the regex, is not reserved, and `skills/<name>` is a folder holding `SKILL.md`.
pub fn is_distributable(repo: &SkillRepo, name: &str) -> bool {
    let dir = repo.skill_dir(name);
    names::is_valid(name)
        && !names::is_reserved(name)
        && std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir())
        && dir.join("SKILL.md").is_file()
}

fn reconcile_tool_dir(dir: &Path, wanted: &[Wanted], report: &mut Report) {
    // The tool directory itself may be missing; its parent chain is never replaced.
    if let Err(e) = std::fs::create_dir_all(dir) {
        report
            .errors
            .push(format!("cannot create {}: {e}", dir.display()));
        return;
    }
    for entry in wanted {
        let path = dir.join(&entry.name);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match crate::fsutil::symlink_atomic(&entry.target, &path) {
                    Ok(()) => report.linked.push(path),
                    Err(e) => report
                        .errors
                        .push(format!("cannot link {}: {e}", path.display())),
                }
            }
            Err(e) => report
                .errors
                .push(format!("cannot inspect {}: {e}", path.display())),
            Ok(_) if ownership::is_owned_link(&path, &entry.target) => {}
            Ok(_) => report.occupied.push(path),
        }
    }
}
