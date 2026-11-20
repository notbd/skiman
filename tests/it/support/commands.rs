//! The skiman steps most tests share, and the backup folder skiman writes.

use std::path::PathBuf;

use super::fs::write;

use super::sandbox::Sandbox;

/// `skiman init` of `sandbox.canon()` without session hooks; it must succeed.
pub fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
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

/// A `PATH` whose `git` logs each call's arguments, one line each, to the
/// returned file, then runs the real git; an empty `PATH` would not do, since
/// some commands go on whatever git does.
pub fn logging_git(sandbox: &Sandbox) -> (String, PathBuf) {
    let real = sandbox
        .command("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout).unwrap();
    let bin = sandbox.root().join("logging-bin");
    let log = sandbox.root().join("git-calls.log");
    write(
        bin.join("git"),
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.trim()
        ),
    );
    std::fs::set_permissions(
        bin.join("git"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    (path, log)
}
