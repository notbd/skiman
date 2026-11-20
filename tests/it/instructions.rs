//! Global instruction files: sync links the shared body to every installed
//! tool's instruction location (spec Ownership, Agent tools), and the shared
//! body rules (spec Rules).

use std::path::PathBuf;

use snapbox::str;

use crate::support::{Sandbox, Tool, assert_symlink, init};

fn shared_body(sandbox: &Sandbox) -> PathBuf {
    sandbox.canon().join("instructions/shared.md")
}

const ALL: [Tool; 3] = [Tool::ClaudeCode, Tool::Codex, Tool::Antigravity];

#[test]
fn an_edit_to_the_shared_body_is_visible_through_every_instruction_location() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let shared = shared_body(&sandbox);
    for tool in ALL {
        assert_symlink(sandbox.instruction_file(tool), &shared, &shared);
    }

    std::fs::write(&shared, "Answer in English.\n").unwrap();
    for tool in ALL {
        assert_eq!(
            std::fs::read_to_string(sandbox.instruction_file(tool)).unwrap(),
            "Answer in English.\n",
            "{tool:?}"
        );
    }
}

#[test]
fn an_existing_regular_claude_md_stays_byte_identical_and_init_points_to_import() {
    let sandbox = Sandbox::new();
    let claude_md = sandbox.instruction_file(Tool::ClaudeCode);
    std::fs::write(&claude_md, b"my own rules\n\xff").unwrap();

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
        .assert()
        .code(3)
        .stdout_eq(str![[r#"
created canon [..]/canon
...
linked [..]/home/.claude/skills/skiman-manual
linked [..]/home/.agents/skills/skiman-manual
linked [..]/home/.gemini/config/skills/skiman-manual
linked [..]/home/.codex/AGENTS.md
linked [..]/home/.gemini/GEMINI.md

"#]])
        .stderr_eq(str![[r#"
warning: instruction file [..]/home/.claude/CLAUDE.md is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`
hint: the canon has no skills yet and the tool directories or instruction locations hold content skiman does not own; bring your existing setup in with `skiman import` (see `skiman import --guide`)

"#]]);
    assert_eq!(std::fs::read(&claude_md).unwrap(), b"my own rules\n\xff");

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(3)
        .stdout_eq("")
        .stderr_eq(str![[r#"
warning: instruction file [..]/home/.claude/CLAUDE.md is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`

"#]]);
    assert_eq!(std::fs::read(&claude_md).unwrap(), b"my own rules\n\xff");

    std::fs::rename(&claude_md, sandbox.root().join("CLAUDE.md.mine")).unwrap();
    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
linked [..]/home/.claude/CLAUDE.md

"#]])
        .stderr_eq("");
    let shared = shared_body(&sandbox);
    assert_symlink(&claude_md, &shared, &shared);
}

#[test]
fn sync_hook_reports_a_refused_location_once() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let agents_md = sandbox.instruction_file(Tool::Codex);
    std::fs::remove_file(&agents_md).unwrap();
    std::fs::write(&agents_md, "codex rules\n").unwrap();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skiman: instruction file [..]/home/.codex/AGENTS.md is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`

"#]])
        .stderr_eq("");
    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    assert_eq!(
        std::fs::read_to_string(&agents_md).unwrap(),
        "codex rules\n"
    );
}

#[test]
fn a_symlink_from_one_instruction_location_to_another_is_not_owned() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let agents_md = sandbox.instruction_file(Tool::Codex);
    let claude_md = sandbox.instruction_file(Tool::ClaudeCode);
    std::fs::remove_file(&agents_md).unwrap();
    std::os::unix::fs::symlink(&claude_md, &agents_md).unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(3)
        .stdout_eq("")
        .stderr_eq(str![[r#"
warning: instruction file [..]/home/.codex/AGENTS.md is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`

"#]]);
    assert_eq!(std::fs::read_link(&agents_md).unwrap(), claude_md);
}

#[test]
fn removing_a_tool_from_tools_enabled_removes_its_owned_instruction_link() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let codex_md = sandbox.instruction_file(Tool::Codex);
    std::fs::remove_file(&codex_md).unwrap();
    std::fs::write(&codex_md, "codex rules\n").unwrap();
    std::fs::write(
        sandbox.canon().join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\"]\n",
    )
    .unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
removed [..]/home/.agents/skills/skiman-manual
removed [..]/home/.gemini/config/skills/skiman-manual
removed [..]/home/.gemini/GEMINI.md

"#]])
        .stderr_eq("");
    assert!(!sandbox.instruction_file(Tool::Antigravity).exists());
    assert_eq!(std::fs::read_to_string(&codex_md).unwrap(), "codex rules\n");
    let shared = shared_body(&sandbox);
    assert_symlink(sandbox.instruction_file(Tool::ClaudeCode), &shared, &shared);
}

#[test]
fn init_with_another_canon_retargets_the_instruction_links() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let other = sandbox.root().join("other-repo");

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&other)
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
created canon [..]/other-repo
...
retargeted [..]/home/.claude/skills/skiman-manual
retargeted [..]/home/.agents/skills/skiman-manual
retargeted [..]/home/.gemini/config/skills/skiman-manual
retargeted [..]/home/.claude/CLAUDE.md
retargeted [..]/home/.codex/AGENTS.md
retargeted [..]/home/.gemini/GEMINI.md

"#]]);
    let shared = other.join("instructions/shared.md");
    for tool in ALL {
        assert_symlink(sandbox.instruction_file(tool), &shared, &shared);
    }
}

fn write_shared_body(sandbox: &Sandbox, bytes: usize) {
    let line = "x".repeat(99) + "\n";
    let mut body = line.repeat(bytes / 100);
    body.push_str(&"y".repeat(bytes % 100));
    assert_eq!(body.len(), bytes);
    std::fs::write(shared_body(sandbox), body).unwrap();
}

fn set_enabled(sandbox: &Sandbox, ids: &str) {
    std::fs::write(
        sandbox.canon().join("skiman.toml"),
        format!("[tools]\nenabled = [{ids}]\n"),
    )
    .unwrap();
}

#[test]
fn the_shared_body_size_warns_above_80_percent_and_fails_above_the_smallest_enabled_limit() {
    let sandbox = Sandbox::new();
    init(&sandbox);

    // Antigravity's 24000 bytes is the smallest limit; 80 % of it is 19200.
    write_shared_body(&sandbox, 19199);
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");

    write_shared_body(&sandbox, 24000);
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq(str![[r#"
warning: instructions/shared.md: shared-body-size: 24000 bytes is at least 80 % of the limit of 24000 bytes (the smallest instruction size limit among enabled tools: antigravity)

"#]]);

    write_shared_body(&sandbox, 24001);
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(1)
        .stdout_eq(str![[r#"
instructions/shared.md: shared-body-size: 24001 bytes is over the limit of 24000 bytes (the smallest instruction size limit among enabled tools: antigravity)

"#]])
        .stderr_eq("");

    // Without antigravity the limit is Codex's 32768 bytes; 80 % of it is 26214.4.
    set_enabled(&sandbox, r#""claude-code", "codex""#);
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    write_shared_body(&sandbox, 26215);
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq(str![[r#"
warning: instructions/shared.md: shared-body-size: 26215 bytes is at least 80 % of the limit of 32768 bytes (the smallest instruction size limit among enabled tools: codex)

"#]]);

    // Claude Code has no known limit.
    write_shared_body(&sandbox, 100_000);
    set_enabled(&sandbox, r#""claude-code""#);
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
}

#[test]
fn an_at_token_at_line_start_or_after_whitespace_outside_code_is_a_violation() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    std::fs::write(
        shared_body(&sandbox),
        "@imports/rules.md
Read\t@docs/style.md first.
Inline `@not-an-import` code and `` a ` @tick `` span.
Mail name@example.com or write to me.　@wide-space

```markdown
@fenced
```

~~~~
@tilde-fenced
~~~
still @inside-the-fence
~~~~
An unclosed `backtick before @open
",
    )
    .unwrap();

    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(1)
        .stdout_eq(str![[r#"
instructions/shared.md:1: at-import: `@imports/rules.md` is read as a file import by Claude Code only; put it in inline code or rewrite it
instructions/shared.md:2: at-import: `@docs/style.md` is read as a file import by Claude Code only; put it in inline code or rewrite it
instructions/shared.md:4: at-import: `@wide-space` is read as a file import by Claude Code only; put it in inline code or rewrite it
instructions/shared.md:15: at-import: `@open` is read as a file import by Claude Code only; put it in inline code or rewrite it

"#]])
        .stderr_eq("");
}

/// A file of the user's that takes the place of an instruction link right
/// before `init` retargets it to another canon is kept, and nothing is
/// left under a temporary name.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_file_that_replaces_an_instruction_link_init_is_about_to_retarget_is_kept() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let other = sandbox.root().join("other-repo");
    let claude_md = sandbox.instruction_file(Tool::ClaudeCode);
    let swap_in = format!("rm '{0}' && printf mine > '{0}'", claude_md.display());
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&other)
        .env(
            "SKIMAN_TEST_HOOK",
            format!("link:take:{}", claude_md.display()),
        )
        .env("SKIMAN_TEST_HOOK_RUN", swap_in)
        .output()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&claude_md).unwrap(), "mine");
    assert!(!claude_md.is_symlink());
    let dir = std::fs::read_dir(claude_md.parent().unwrap()).unwrap();
    let names: Vec<String> = dir
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n.contains(".skiman-tmp-")),
        "{names:?}"
    );
    let shared = other.join("instructions/shared.md");
    assert_symlink(sandbox.instruction_file(Tool::Codex), &shared, &shared);
}

/// On a filesystem with a swapping rename, a retargeted instruction link is
/// never missing: at the step between swapping it out and removing the old
/// one, the path is already the new link.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_retargeted_instruction_link_is_never_missing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let other = sandbox.root().join("other-repo");
    let claude_md = sandbox.instruction_file(Tool::ClaudeCode);
    let shared = other.join("instructions/shared.md");
    let check = format!(
        "test \"$(readlink '{}')\" = '{}'",
        claude_md.display(),
        shared.display()
    );
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&other)
        .env(
            "SKIMAN_TEST_HOOK",
            format!("link:aside:{}", claude_md.display()),
        )
        .env("SKIMAN_TEST_HOOK_RUN", check)
        .assert()
        .code(0);
    assert_symlink(&claude_md, &shared, &shared);
}

/// A retarget whose swapped-out link turns out not to be skiman's swaps
/// back; when something else, B, took the path in between, B comes out of
/// that swap back and stays under the temporary name, never removed, and
/// the error says where. B a file and a folder.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn what_comes_out_of_a_retarget_swapped_back_is_never_removed() {
    for b in [
        "printf B > \"$L\"",
        "mkdir \"$L\" && printf B > \"$L/b.md\"",
    ] {
        let sandbox = Sandbox::new();
        init(&sandbox);
        let other = sandbox.root().join("other-repo");
        let claude_md = sandbox.instruction_file(Tool::ClaudeCode);
        // Right after the swap: the link swapped out becomes the user's A,
        // and the new link at the path is replaced by B.
        let run = format!(
            "L='{}'; T=$(ls -d \"$(dirname \"$L\")\"/.CLAUDE.md.skiman-tmp-*); rm \"$T\" && printf A > \"$T\" && rm \"$L\" && {b}",
            claude_md.display()
        );
        let output = sandbox
            .skiman()
            .args(["init", "--session-hooks", "none"])
            .arg(&other)
            .env(
                "SKIMAN_TEST_HOOK",
                format!("link:aside:{}", claude_md.display()),
            )
            .env("SKIMAN_TEST_HOOK_RUN", run)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            std::fs::read_to_string(&claude_md).unwrap(),
            "A",
            "{stderr}"
        );
        let dir = claude_md.parent().unwrap();
        let aside: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".skiman-tmp-"))
            .collect();
        let [aside] = &aside[..] else {
            panic!("{aside:?}");
        };
        let b = match aside.is_dir() {
            true => aside.join("b.md"),
            false => aside.clone(),
        };
        assert_eq!(std::fs::read_to_string(b).unwrap(), "B");
        let said = format!("what was there is at {}", aside.display());
        assert!(stderr.contains(&said), "{stderr}");
    }
}

/// What sync and doctor say about the shared body, in its words.
fn shared_body_line(sandbox: &Sandbox, why: &str) -> String {
    let shared = sandbox.canon().join("instructions/shared.md");
    format!(
        "the shared body {} {why}, so every instruction file linked to it leads nowhere; make it a file (it may be empty)",
        shared.display()
    )
}

/// Without `instructions/shared.md` every instruction link leads nowhere:
/// `sync` and doctor say so (exit 0; an empty file is fine), and say it is
/// not a file when a folder is there; `sync --hook` reads no more than
/// before and stays silent.
#[test]
fn a_missing_shared_body_is_named_by_sync_and_doctor() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let shared = sandbox.canon().join("instructions/shared.md");
    std::fs::remove_file(&shared).unwrap();
    let line = shared_body_line(&sandbox, "is missing");

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("");
    let sync = sandbox.skiman().arg("sync").assert().code(0);
    let stderr = String::from_utf8(sync.get_output().stderr.clone()).unwrap();
    assert_eq!(stderr, format!("warning: {line}\n"));
    let doctor = sandbox.skiman().arg("doctor").assert().code(0);
    let stdout = String::from_utf8(doctor.get_output().stdout.clone()).unwrap();
    assert!(stdout.lines().any(|l| l == line), "{stdout}");
    std::fs::create_dir(&shared).unwrap();
    let sync = sandbox.skiman().arg("sync").assert().code(0);
    let stderr = String::from_utf8(sync.get_output().stderr.clone()).unwrap();
    let line = shared_body_line(&sandbox, "is not a file");
    assert_eq!(stderr, format!("warning: {line}\n"));
    std::fs::remove_dir(&shared).unwrap();
    std::fs::write(&shared, "").unwrap();
    sandbox.skiman().arg("sync").assert().code(0).stderr_eq("");
}

/// With no installed tool enabled, no instruction link leads to the shared
/// body, so a missing one is not named, though a tool that is not enabled
/// is on this machine.
#[test]
fn a_missing_shared_body_is_not_named_when_no_enabled_tool_is_installed() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let manifest = sandbox.canon().join("skiman.toml");
    std::fs::write(&manifest, "[tools]\nenabled = [\"claude-code\"]\n").unwrap();
    std::fs::remove_dir_all(sandbox.presence_dir(Tool::ClaudeCode)).unwrap();
    std::fs::remove_file(sandbox.canon().join("instructions/shared.md")).unwrap();

    let sync = sandbox.skiman().arg("sync").assert().code(0);

    let stderr = String::from_utf8(sync.get_output().stderr.clone()).unwrap();
    assert!(!stderr.contains("shared body"), "{stderr}");
}

/// The missing shared body is named by `skiman sync` only, so it is not
/// recorded for the hook: `sync`, `sync --hook`, `sync` leave the record of
/// what was reported byte for byte as it is.
#[test]
fn a_missing_shared_body_is_not_recorded_for_the_hook() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    std::fs::remove_file(sandbox.canon().join("instructions/shared.md")).unwrap();
    let reported = sandbox.state_home().join("skiman/reported.toml");
    sandbox.skiman().arg("sync").assert().code(0);
    let before = std::fs::read(&reported).ok();

    for args in [&["sync", "--hook"][..], &["sync"]] {
        sandbox.skiman().args(args).assert().code(0);
        assert_eq!(std::fs::read(&reported).ok(), before, "after {args:?}");
    }
}
