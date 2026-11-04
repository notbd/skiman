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
    let violations = rules::check_all(&WorkingTree::new(repo.root()))
        .map_err(|e| Failure::operational(format!("cannot read {}: {e}", repo.root().display())))?;
    for violation in &violations {
        println!("{violation}");
    }
    Ok(rules::exit_of(&violations))
}
