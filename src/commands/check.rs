//! `skiman check [--staged]`: report every rule violation in the working tree
//! of the skill repo, or in its staged snapshot. Read-only; takes no lock.

use crate::Exit;
use crate::failure::{Failure, finish};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::rules;
use crate::staged::StagedTree;
use crate::tree::WorkingTree;

pub fn run(staged: bool) -> Exit {
    finish(check(staged))
}

fn check(staged: bool) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let report = if staged {
        let tree = StagedTree::read(repo.root()).map_err(|e| {
            Failure::operational(format!(
                "cannot read the staged snapshot of {}: {e}",
                repo.root().display()
            ))
        })?;
        rules::check_repo(&tree)
    } else {
        rules::check_repo(&WorkingTree::new(repo.root()))
    }
    .map_err(|e| Failure::operational(format!("cannot read {}: {e}", repo.root().display())))?;
    for violation in &report.violations {
        println!("{violation}");
    }
    for unused in &report.unused_allows {
        println!("{unused}");
    }
    if let Some(hint) = rules::allow_hint(&report.violations) {
        println!("\n{hint}");
    }
    Ok(rules::exit_of(&report.violations))
}
