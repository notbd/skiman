//! A write command killed at a precise step, then run again (ADR 0005: a
//! command stopped part-way finishes when it is run again).
//!
//! The test-only hook does the killing: `SKIMAN_TEST_HOOK` names
//! `<scope>:<point>` and `SKIMAN_TEST_HOOK_RUN` is the shell command debug
//! builds run there; `kill -9 $PPID` kills skiman.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;

use super::sandbox::Sandbox;
use super::tree::tree;

/// Runs `skiman <args>` and kills it with signal 9 at `hook`.
#[track_caller]
pub fn killed_at(sandbox: &Sandbox, args: &[String], hook: &str) {
    let output = sandbox
        .skiman()
        .args(args)
        .env("SKIMAN_TEST_HOOK", hook)
        .env("SKIMAN_TEST_HOOK_RUN", "kill -9 $PPID")
        .output()
        .unwrap();
    assert_eq!(
        output.status.signal(),
        Some(9),
        "{hook}: {:?}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// [`tree`] of `dir` with each path below `prefix` and the sandbox root
/// replaced, so two sandboxes compare equal.
pub fn normalised(sandbox: &Sandbox, prefix: &str, dir: &Path) -> BTreeMap<String, String> {
    let root = sandbox.root().display().to_string();
    tree(dir)
        .into_iter()
        .map(|(path, value)| {
            let key = Path::new(prefix).join(path).display().to_string();
            (key, value.replace(&root, "<root>"))
        })
        .collect()
}

/// The skill repo and the home folder (tool directories, instruction files,
/// machine state). Backups and the git cache are left out: a killed run
/// leaves a backup folder of its own, and fetching fills the cache.
pub fn final_state(sandbox: &Sandbox) -> BTreeMap<String, String> {
    let mut state = normalised(sandbox, "repo", &sandbox.skill_repo());
    state.extend(normalised(sandbox, "home", &sandbox.home()));
    state.retain(|key, _| {
        !key.starts_with("home/.local/state/skiman/backups")
            && !key.starts_with("home/.local/state/skiman/cache")
    });
    state
}

/// For each of `hooks`: a new sandbox made ready by `setup`, which returns
/// the command's arguments; the command killed at the hook (`<root>` in it
/// stands for the sandbox's root); then the same command run again, which
/// must exit 0 and end in the [`final_state`] of a run that was never
/// killed. Panics naming every hook that ends otherwise.
#[track_caller]
pub fn assert_each_kill_finishes(setup: impl Fn(&Sandbox) -> Vec<String>, hooks: &[&str]) {
    let sandbox = Sandbox::new();
    let args = setup(&sandbox);
    let output = sandbox.skiman().args(&args).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = final_state(&sandbox);
    let failures: Vec<String> = hooks
        .iter()
        .filter_map(|hook| {
            let sandbox = Sandbox::new();
            let args = setup(&sandbox);
            let root = sandbox.root().display().to_string();
            killed_at(&sandbox, &args, &hook.replace("<root>", &root));
            let output = sandbox.skiman().args(&args).output().unwrap();
            let mut problems = Vec::new();
            if output.status.code() != Some(0) {
                problems.push(format!(
                    "exit {:?}: {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let state = final_state(&sandbox);
            let keys: BTreeSet<&String> = expected.keys().chain(state.keys()).collect();
            for key in keys {
                if expected.get(key) != state.get(key) {
                    problems.push(format!(
                        "{key}: expected {:?}, found {:?}",
                        expected.get(key),
                        state.get(key)
                    ));
                }
            }
            (!problems.is_empty())
                .then(|| format!("killed at {hook}:\n  {}", problems.join("\n  ")))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
