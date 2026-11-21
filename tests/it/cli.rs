//! The CLI skeleton: version, help, usage errors, and output to a closed pipe.

use std::process::Stdio;

use snapbox::str;

use crate::support::{Sandbox, read};

#[test]
fn version_prints_the_version_and_exits_0() {
    Sandbox::new()
        .skiman()
        .arg("--version")
        .assert()
        .code(0)
        .stdout_eq(format!("skiman {}\n", env!("CARGO_PKG_VERSION")))
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

/// A pipe whose read end is already closed: every write to it fails with `EPIPE`.
fn closed_pipe() -> std::io::PipeWriter {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    writer
}

#[test]
fn add_with_stdout_closed_still_installs_the_skill_and_exits_0() {
    let sandbox = Sandbox::new();
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
        .assert()
        .code(0);
    let commit = sandbox
        .upstream("acme/pdf")
        .write(
            "SKILL.md",
            "---\nname: pdf\ndescription: Fill PDF forms\n---\n\n# pdf\n",
        )
        .commit("v1");

    let status = sandbox
        .std_command(env!("CARGO_BIN_EXE_skiman"))
        .args(["add", "acme/pdf", "--yes"])
        .stdin(Stdio::null())
        .stdout(closed_pipe())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    let repo = sandbox.canon();
    assert!(repo.join("skills/pdf/SKILL.md").is_file());
    assert!(repo.join("upstream/pdf/SKILL.md").is_file());
    assert!(read(repo.join("skiman.toml")).contains("[skills.pdf.source]"));
    assert!(read(repo.join("skiman.lock")).contains(&format!("commit = \"{commit}\"")));
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn a_refused_command_with_stderr_closed_still_exits_1() {
    let sandbox = Sandbox::new();
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
        .assert()
        .code(0);

    let status = sandbox
        .std_command(env!("CARGO_BIN_EXE_skiman"))
        .args(["remove", "nosuch", "--yes"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(closed_pipe())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}
