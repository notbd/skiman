//! `skiman check [--staged]`: report every rule violation in the working tree
//! of the skill repo, or in its staged snapshot, on stdout, and warnings on
//! stderr. Read-only; takes no lock.

use std::path::PathBuf;
use std::process::Command;

use crate::Exit;
use crate::failure::{Failure, finish};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::rules;
use crate::staged::StagedTree;
use crate::tree::{Tree, WorkingTree};

pub fn run(staged: bool) -> Exit {
    finish(check(staged))
}

fn check(staged: bool) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    if staged && let Some(other) = hook_of_another_repo(&repo) {
        println!(
            "skiman: {} is not the skill repo ({}), so its skiman pre-commit hook checks nothing",
            other.display(),
            repo.root().display()
        );
        return Ok(Exit::Success);
    }
    let tree: Box<dyn Tree> = if staged {
        Box::new(StagedTree::read(repo.root()).map_err(|e| {
            Failure::operational(format!(
                "cannot read the staged snapshot of {}: {e}",
                repo.root().display()
            ))
        })?)
    } else {
        Box::new(WorkingTree::new(repo.root()))
    };
    let io = |e| Failure::operational(format!("cannot read {}: {e}", repo.root().display()));
    let report = rules::check_repo(tree.as_ref()).map_err(io)?;
    for violation in &report.violations {
        println!("{violation}");
    }
    for unused in &report.unused_allows {
        println!("{unused}");
    }
    if let Some(hint) = rules::allow_hint(&report.violations) {
        println!("\n{hint}");
    }
    for warning in rules::warnings_all(tree.as_ref()).map_err(io)? {
        eprintln!("warning: {warning}");
    }
    Ok(rules::exit_of(&report.violations))
}

/// Run from a git hook (`git commit` sets `GIT_INDEX_FILE`) of a repository
/// that is not the skill repo, e.g. a previous skill repo that still has
/// skiman's hooks: that repository's top level.
fn hook_of_another_repo(repo: &SkillRepo) -> Option<PathBuf> {
    std::env::var_os("GIT_INDEX_FILE")?;
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let top = PathBuf::from(
        String::from_utf8(output.stdout)
            .ok()?
            .trim_end_matches('\n'),
    );
    let top = top.canonicalize().unwrap_or(top);
    (top != repo.root()).then_some(top)
}
