//! The skiman steps most tests share, and the backup folder skiman writes.

use std::path::PathBuf;

use super::sandbox::Sandbox;

/// `skiman init` of `sandbox.skill_repo()` without session hooks; it must succeed.
pub fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

/// `skiman check` reports nothing.
pub fn check_is_clean(sandbox: &Sandbox) {
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

/// The folder skiman moves replaced content into, one subfolder per command.
pub fn backups(sandbox: &Sandbox) -> PathBuf {
    sandbox.state_home().join("skiman/backups")
}

/// Every folder under [`backups`], sorted; empty when there is none.
pub fn backup_dirs(sandbox: &Sandbox) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(backups(sandbox)) {
        Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    };
    dirs.sort();
    dirs
}

/// The only folder under [`backups`]; fails unless there is exactly one.
#[track_caller]
pub fn only_backup(sandbox: &Sandbox) -> PathBuf {
    let [dir] = backup_dirs(sandbox)
        .try_into()
        .unwrap_or_else(|dirs| panic!("one backup folder: {dirs:?}"));
    dir
}
