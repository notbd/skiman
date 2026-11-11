//! Self-tests of the black-box harness in `tests/it/support`.

use std::collections::BTreeMap;

use crate::support::{Sandbox, Tool, ToolSetup, assert_dangling_symlink, assert_symlink};

fn env_seen_by(sandbox: &Sandbox) -> BTreeMap<String, String> {
    let output = sandbox.command("/usr/bin/env").output().expect("run env");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let (k, v) = line.split_once('=').unwrap();
            (k.to_owned(), v.to_owned())
        })
        .collect()
}

#[test]
fn commands_see_only_the_sandbox_environment() {
    let sandbox = Sandbox::new();
    let env = env_seen_by(&sandbox);
    let root = sandbox.root().to_str().unwrap();

    let keys: Vec<&str> = env.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
            "HOME",
            "LC_ALL",
            "PATH",
            "TMPDIR",
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
        ],
        "nothing else is inherited from the test process"
    );
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "GIT_CONFIG_GLOBAL",
        "TMPDIR",
    ] {
        assert!(
            env[key].starts_with(&format!("{root}/")),
            "{key}={} is outside the sandbox {root}",
            env[key]
        );
    }
    assert_eq!(env["GIT_CONFIG_NOSYSTEM"], "1");
    assert_ne!(
        Some(env["HOME"].as_str()),
        std::env::var("HOME").ok().as_deref(),
        "HOME is not the real home"
    );
}

#[test]
fn sandbox_creates_the_agent_tool_directories_it_is_asked_for() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Codex, ToolSetup::Absent)
        .tool(Tool::Antigravity, ToolSetup::PresenceOnly)
        .build();

    assert!(sandbox.tool_dir(Tool::ClaudeCode).is_dir());
    assert!(!sandbox.presence_dir(Tool::Codex).exists());
    assert!(!sandbox.tool_dir(Tool::Codex).exists());
    assert!(sandbox.presence_dir(Tool::Antigravity).is_dir());
    assert!(!sandbox.tool_dir(Tool::Antigravity).exists());
    assert_eq!(
        sandbox.tool_dir(Tool::Codex),
        sandbox.home().join(".agents/skills")
    );
    assert!(!sandbox.skill_repo().exists());
}

#[test]
fn confined_commands_cannot_write_outside_the_sandbox() {
    let sandbox = Sandbox::new();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("leak");
    let inside_file = sandbox.home().join("ok");

    sandbox
        .confined("/usr/bin/touch")
        .arg(&inside_file)
        .assert()
        .success();
    assert!(inside_file.exists());

    sandbox
        .confined("/usr/bin/touch")
        .arg(&outside_file)
        .assert()
        .interrupted();
    assert!(
        !outside_file.exists(),
        "the confinement must block the write"
    );
}

#[test]
fn skiman_writes_nothing_outside_the_sandbox() {
    let sandbox = Sandbox::new();
    // A write outside the sandbox would kill skiman, failing these exit-code checks.
    sandbox.skiman_confined().arg("--version").assert().code(0);
    sandbox.skiman_confined().arg("--help").assert().code(0);
    sandbox.skiman_confined().arg("frobnicate").assert().code(2);
}

#[test]
fn github_urls_resolve_to_local_upstreams_without_network() {
    let sandbox = Sandbox::new();
    let upstream = sandbox.upstream("acme/skills");
    upstream.write("skills/pdf/SKILL.md", "---\nname: pdf\n---\nv1\n");
    let v1 = upstream.commit("v1");
    upstream.tag("v1.0");
    upstream.switch("feature/new-pdf");
    upstream.write("skills/pdf/SKILL.md", "---\nname: pdf\n---\nv2\n");
    let v2 = upstream.commit("v2");
    upstream.switch("main");
    assert_eq!(upstream.url(), "https://github.com/acme/skills.git");

    // Confinement denies all network access, so only the rewrite can succeed.
    let refs = sandbox
        .confined("git")
        .args(["ls-remote", "--", &upstream.url()])
        .output()
        .unwrap();
    assert!(
        refs.status.success(),
        "{}",
        String::from_utf8_lossy(&refs.stderr)
    );
    let refs = String::from_utf8(refs.stdout).unwrap();
    assert!(refs.contains(&format!("{v1}\trefs/heads/main")), "{refs}");
    assert!(
        refs.contains(&format!("{v2}\trefs/heads/feature/new-pdf")),
        "{refs}"
    );
    assert!(refs.contains(&format!("{v1}\trefs/tags/v1.0")), "{refs}");
    assert!(
        refs.contains(&format!("{v1}\tHEAD")),
        "default branch is main: {refs}"
    );

    let clone = sandbox.root().join("clone");
    sandbox
        .confined("git")
        .args(["clone", "--quiet", "--branch", "feature/new-pdf", "--"])
        .arg(upstream.url())
        .arg(&clone)
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(clone.join("skills/pdf/SKILL.md")).unwrap(),
        "---\nname: pdf\n---\nv2\n"
    );
}

#[test]
fn symlink_assertion_checks_lexical_and_resolved_target() {
    let sandbox = Sandbox::new();
    let skill = sandbox.skill_repo().join("skills/pdf");
    std::fs::create_dir_all(&skill).unwrap();
    // An alias of the skill repo: the lexical target goes through it, the
    // resolved target does not.
    let alias = sandbox.root().join("repo-alias");
    std::os::unix::fs::symlink(sandbox.skill_repo(), &alias).unwrap();
    let link = sandbox.tool_dir(Tool::ClaudeCode).join("pdf");
    std::os::unix::fs::symlink(alias.join("skills/pdf"), &link).unwrap();

    assert_symlink(&link, alias.join("skills/pdf"), &skill);

    let wrong_lexical = std::panic::catch_unwind(|| assert_symlink(&link, &skill, &skill)).is_err();
    assert!(wrong_lexical, "a different link text must fail");
    let other = sandbox.skill_repo().join("skills/other");
    std::fs::create_dir_all(&other).unwrap();
    let wrong_resolved =
        std::panic::catch_unwind(|| assert_symlink(&link, alias.join("skills/pdf"), &other))
            .is_err();
    assert!(wrong_resolved, "a different resolved target must fail");

    std::fs::remove_dir(&skill).unwrap();
    assert_dangling_symlink(&link, alias.join("skills/pdf"));
}

#[test]
fn pty_drives_a_scripted_interaction() {
    let sandbox = Sandbox::new();
    let mut sh = sandbox.std_command("/bin/sh");
    sh.args([
        "-c",
        r#"[ -t 0 ] && echo "stdin is a terminal"; printf 'Name? '; read name; echo "hello $name"; exit 3"#,
    ]);
    let mut pty = crate::support::Pty::spawn(sh);
    pty.expect("stdin is a terminal");
    pty.expect("Name? ");
    pty.send_line("Ada");
    pty.expect("hello Ada");
    let (code, _) = pty.wait();
    assert_eq!(code, 3);

    let mut skiman = sandbox.skiman_pty(["--version"]);
    skiman.expect("skiman 0.1.0");
    assert_eq!(skiman.wait().0, 0);
}
