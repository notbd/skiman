//! The CLI skeleton: version, help and usage errors.

mod support;

use snapbox::str;
use support::Sandbox;

#[test]
fn version_prints_the_version_and_exits_0() {
    Sandbox::new()
        .skiman()
        .arg("--version")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skiman 0.1.0

"#]])
        .stderr_eq("");
}

#[test]
fn help_prints_usage_on_stdout_and_exits_0() {
    Sandbox::new()
        .skiman()
        .arg("--help")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
...
Usage: skiman[..]
...
"#]])
        .stderr_eq("");
}

#[test]
fn unknown_command_exits_2_with_usage_on_stderr() {
    Sandbox::new()
        .skiman()
        .arg("frobnicate")
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: [..]'frobnicate'[..]
...
Usage: skiman[..]
...
"#]]);
}

#[test]
fn unknown_flag_exits_2_with_usage_on_stderr() {
    Sandbox::new()
        .skiman()
        .arg("--frobnicate")
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: unexpected argument '--frobnicate' found
...
Usage: skiman[..]
...
"#]]);
}

#[test]
fn no_command_exits_2_with_usage_on_stderr() {
    Sandbox::new()
        .skiman()
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq(str![[r#"
...
Usage: skiman[..]
...
"#]]);
}
