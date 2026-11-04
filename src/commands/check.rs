//! `skiman check`: report every rule violation in the working tree of the
//! skill repo on stdout, and warnings on stderr. Read-only; takes no lock.

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
    let tree = WorkingTree::new(repo.root());
    let io = |e| Failure::operational(format!("cannot read {}: {e}", repo.root().display()));
    let violations = rules::check_all(&tree).map_err(io)?;
    for violation in &violations {
        println!("{violation}");
    }
    for warning in rules::warnings_all(&tree).map_err(io)? {
        eprintln!("warning: {warning}");
    }
    Ok(rules::exit_of(&violations))
}
