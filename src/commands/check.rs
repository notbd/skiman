//! `skiman check [--staged]`: report every rule violation in the working tree
//! of the skill repo, or in its staged snapshot, on stdout, and warnings on
//! stderr. Read-only; takes no lock.

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
