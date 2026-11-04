//! `skiman sync [--hook]`: reconcile distribution. The loop itself is `crate::sync`.

use crate::Exit;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::{Acquire, RepoLock};
use crate::sync::{self, Output};

pub fn run(hook: bool) -> Exit {
    let output = if hook { Output::Hook } else { Output::Normal };
    let how = if hook {
        Acquire::TryOnce
    } else {
        Acquire::Wait
    };
    let paths = match Paths::from_env() {
        Ok(paths) => paths,
        Err(e) => return sync::report_failure(e.into(), output),
    };
    let repo = match SkillRepo::from_config(&paths) {
        Ok(repo) => repo,
        Err(failure) => return sync::report_failure(failure, output),
    };
    match RepoLock::acquire(&repo, how) {
        Ok(Some(lock)) => sync::run(&paths, &repo, &lock, output),
        // Busy: another command is syncing; `--hook` exits 0 silently.
        Ok(None) => Exit::Success,
        Err(failure) => sync::report_failure(failure, output),
    }
}
