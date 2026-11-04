//! Failure injection for black-box tests: lets a test act at a precise step
//! of a write command (for example edit a skill between two re-hashes).
//! Compiled only into debug builds; release builds never read the variables.
//!
//! `SKIMAN_TEST_HOOK=<skill>:<point>` selects the step and
//! `SKIMAN_TEST_HOOK_RUN` is the `/bin/sh -c` command run there.

/// Runs the test hook when it names `skill` and `point`.
#[cfg(debug_assertions)]
pub fn at(skill: &str, point: &str) {
    let Some(at) = std::env::var_os("SKIMAN_TEST_HOOK") else {
        return;
    };
    if at.to_str() != Some(&format!("{skill}:{point}")) {
        return;
    }
    let command = std::env::var_os("SKIMAN_TEST_HOOK_RUN").unwrap_or_default();
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .status()
        .expect("run SKIMAN_TEST_HOOK_RUN");
    assert!(status.success(), "SKIMAN_TEST_HOOK_RUN failed: {status}");
}

#[cfg(not(debug_assertions))]
pub fn at(_skill: &str, _point: &str) {}
