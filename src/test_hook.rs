//! Failure injection for black-box tests: lets a test act at a precise step
//! of a write command (for example edit a skill right before it is written).
//! Compiled only into debug builds; release builds never read the variables.
//!
//! `SKIMAN_TEST_HOOK=<scope>:<point>` selects the step (the scope is a skill
//! name, or a command name such as `import`) and
//! `SKIMAN_TEST_HOOK_RUN` is the `/bin/sh -c` command run there.

/// Runs the test hook when it names `scope` and `point`.
#[cfg(debug_assertions)]
pub fn at(scope: &str, point: &str) {
    let Some(at) = std::env::var_os("SKIMAN_TEST_HOOK") else {
        return;
    };
    if at.to_str() != Some(&format!("{scope}:{point}")) {
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
pub fn at(_scope: &str, _point: &str) {}
