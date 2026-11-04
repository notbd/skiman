//! `skiman check`: report every rule violation in the working tree of the
//! skill repo. Read-only; takes no lock.

use crate::Exit;
use crate::failure::{Failure, finish};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::rules;
use crate::tree::WorkingTree;

pub fn run() -> Exit {
    finish(check())
}

fn check() -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let report = rules::check_repo(&WorkingTree::new(repo.root()))
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
