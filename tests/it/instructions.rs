//! Global instruction files: sync links the shared body to every installed
//! tool's instruction location (spec Ownership, Agent tools), and the shared
//! body rules (spec Rules).

use std::path::PathBuf;

use snapbox::str;

use crate::support::{Sandbox, Tool, assert_symlink, init};

fn shared_body(sandbox: &Sandbox) -> PathBuf {
    sandbox.skill_repo().join("instructions/shared.md")
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
        .arg(sandbox.skill_repo())
        .assert()
        .code(3)
        .stdout_eq(str![[r#"
created skill repo [..]/skill-repo
...
linked [..]/home/.claude/skills/managing-skills
linked [..]/home/.agents/skills/managing-skills
linked [..]/home/.gemini/config/skills/managing-skills
linked [..]/home/.codex/AGENTS.md
linked [..]/home/.gemini/GEMINI.md

"#]])
        .stderr_eq(str![[r#"
warning: instruction file [..]/home/.claude/CLAUDE.md is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`
hint: the skill repo has no skills yet and the tool directories or instruction locations hold content skiman does not own; bring your existing setup in with `skiman import` (see `skiman import --guide`)

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
        sandbox.skill_repo().join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\"]\n",
    )
    .unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
removed [..]/home/.agents/skills/managing-skills
removed [..]/home/.gemini/config/skills/managing-skills
removed [..]/home/.gemini/GEMINI.md

"#]])
        .stderr_eq("");
    assert!(!sandbox.instruction_file(Tool::Antigravity).exists());
    assert_eq!(std::fs::read_to_string(&codex_md).unwrap(), "codex rules\n");
    let shared = shared_body(&sandbox);
    assert_symlink(sandbox.instruction_file(Tool::ClaudeCode), &shared, &shared);
}

#[test]
fn init_with_another_skill_repo_retargets_the_instruction_links() {
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
created skill repo [..]/other-repo
...
retargeted [..]/home/.claude/skills/managing-skills
retargeted [..]/home/.agents/skills/managing-skills
retargeted [..]/home/.gemini/config/skills/managing-skills
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
        sandbox.skill_repo().join("skiman.toml"),
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
