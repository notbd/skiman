//! `skiman init --session-hooks`: session-start hook entries in the agent tools' settings files.

mod support;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use snapbox::str;
use support::{Sandbox, Tool, ToolSetup};

fn claude_settings(sandbox: &Sandbox) -> PathBuf {
    sandbox.home().join(".claude/settings.json")
}

fn codex_hooks(sandbox: &Sandbox) -> PathBuf {
    sandbox.home().join(".codex/hooks.json")
}

fn read_json(path: &Path) -> Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn write_json(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap() + "\n").unwrap();
}

/// The group skiman adds, for the binary the tests run.
fn skiman_group() -> Value {
    json!({
        "matcher": "startup|resume",
        "hooks": [{
            "type": "command",
            "command": format!("{} sync --hook", env!("CARGO_BIN_EXE_skiman")),
            "timeout": 10
        }]
    })
}

/// A settings file with unrelated keys, an existing SessionStart group and other events.
fn user_settings() -> Value {
    json!({
        "model": "opus",
        "permissions": { "allow": ["Bash(ls:*)"], "deny": [] },
        "hooks": {
            "SessionStart": [{
                "hooks": [{ "type": "command", "command": "~/.claude/hooks/boot.py" }]
            }],
            "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }]
        },
        "statusLine": { "type": "command", "command": "status.sh", "padding": 0 }
    })
}

fn with_skiman_group(mut settings: Value) -> Value {
    settings["hooks"]["SessionStart"]
        .as_array_mut()
        .unwrap()
        .push(skiman_group());
    settings
}

fn init(sandbox: &Sandbox, session_hooks: &str) -> snapbox::cmd::OutputAssert {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", session_hooks])
        .arg(sandbox.skill_repo())
        .assert()
}

#[test]
fn session_hooks_claude_code_and_codex_adds_one_claude_code_entry_and_keeps_every_other_key() {
    let sandbox = Sandbox::new();
    write_json(&claude_settings(&sandbox), &user_settings());
    let codex_before = "{\n    \"hooks\": {}\n}\n";
    std::fs::write(codex_hooks(&sandbox), codex_before).unwrap();

    init(&sandbox, "claude-code,codex")
        .code(0)
        .stdout_eq(str![[r#"
created skill repo [..]/skill-repo
...
added the session-start hook entry to [..]/home/.claude/settings.json

"#]])
        .stderr_eq(str![[r#"
warning: session refresh for Codex is not available until it is confirmed (Phase 1 experiment 4); skiman adds no entry to [..]/home/.codex/hooks.json

"#]]);

    assert_eq!(
        read_json(&claude_settings(&sandbox)),
        with_skiman_group(user_settings())
    );
    assert_eq!(
        std::fs::read_to_string(codex_hooks(&sandbox)).unwrap(),
        codex_before
    );

    let settings_bytes = std::fs::read(claude_settings(&sandbox)).unwrap();
    init(&sandbox, "claude-code,codex")
        .code(0)
        .stdout_eq("")
        .stderr_eq("warning: session refresh for Codex is not available until it is confirmed (Phase 1 experiment 4); skiman adds no entry to [..]/home/.codex/hooks.json\n");
    assert_eq!(
        std::fs::read(claude_settings(&sandbox)).unwrap(),
        settings_bytes
    );
}

#[test]
fn the_claude_code_entry_uses_matcher_startup_resume_and_calls_the_recorded_binary() {
    let sandbox = Sandbox::new();

    init(&sandbox, "claude-code").code(0).stderr_eq("");

    // Missing settings file in an existing ~/.claude: created with only the entry.
    assert_eq!(
        read_json(&claude_settings(&sandbox)),
        json!({
            "hooks": {
                "SessionStart": [{
                    "matcher": "startup|resume",
                    "hooks": [{
                        "type": "command",
                        "command": format!("{} sync --hook", env!("CARGO_BIN_EXE_skiman")),
                        "timeout": 10
                    }]
                }]
            }
        })
    );
    assert!(!codex_hooks(&sandbox).exists());
}

#[test]
fn a_symlinked_settings_file_stays_a_symlink_and_its_target_holds_the_entry() {
    let sandbox = Sandbox::new();
    let dotfiles = sandbox.root().join("dotfiles");
    std::fs::create_dir(&dotfiles).unwrap();
    let target = dotfiles.join("claude-settings.json");
    write_json(&target, &user_settings());
    std::os::unix::fs::symlink(&target, claude_settings(&sandbox)).unwrap();

    init(&sandbox, "claude-code").code(0);

    let link = std::fs::read_link(claude_settings(&sandbox)).unwrap();
    assert_eq!(link, target);
    assert_eq!(read_json(&target), with_skiman_group(user_settings()));
    let leftovers: Vec<_> = std::fs::read_dir(&dotfiles)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["claude-settings.json"]);
}

#[test]
fn rewriting_a_settings_file_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    write_json(&claude_settings(&sandbox), &user_settings());
    std::fs::set_permissions(
        claude_settings(&sandbox),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();

    init(&sandbox, "claude-code").code(0);

    let mode = std::fs::metadata(claude_settings(&sandbox))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(
        read_json(&claude_settings(&sandbox)),
        with_skiman_group(user_settings())
    );
}

#[test]
fn a_settings_file_that_is_not_valid_json_stays_byte_identical_and_init_exits_4_with_the_entry() {
    let sandbox = Sandbox::new();
    let broken = "{ \"model\": \"opus\", // a comment\n}\n";
    std::fs::write(claude_settings(&sandbox), broken).unwrap();

    init(&sandbox, "claude-code").code(4).stderr_eq(str![[r#"
error: [..]/home/.claude/settings.json is not valid JSON (key must be a string at line 1 column 20), so skiman did not change it; add this entry to `hooks.SessionStart` by hand:
{
  "matcher": "startup|resume",
  "hooks": [
    {
      "type": "command",
      "command": "[..]/skiman sync --hook",
      "timeout": 10
    }
  ]
}

"#]]);

    assert_eq!(
        std::fs::read_to_string(claude_settings(&sandbox)).unwrap(),
        broken
    );
    // The rest of init still happened.
    assert!(sandbox.skill_repo().join("skiman.toml").is_file());
}

#[test]
fn a_settings_file_whose_session_start_is_not_a_list_is_left_alone_and_init_exits_4() {
    let sandbox = Sandbox::new();
    let odd = "{\"hooks\": {\"SessionStart\": {\"matcher\": \"startup\"}}}\n";
    std::fs::write(claude_settings(&sandbox), odd).unwrap();

    init(&sandbox, "claude-code").code(4).stderr_eq(str![[r#"
error: [..]/home/.claude/settings.json: `hooks.SessionStart` is not a list, so skiman did not change it; add this entry to `hooks.SessionStart` by hand:
...
"#]]);

    assert_eq!(
        std::fs::read_to_string(claude_settings(&sandbox)).unwrap(),
        odd
    );
}

#[test]
fn an_older_skiman_entry_is_replaced_so_the_file_holds_exactly_one() {
    let sandbox = Sandbox::new();
    let mut settings = user_settings();
    // A hand-added skiman hook inside the user's own group, and an old binary path.
    settings["hooks"]["SessionStart"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "type": "command", "command": "/old/bin/skiman sync --hook" }));
    settings["hooks"]["SessionStart"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "matcher": "startup",
            "hooks": [{ "type": "command", "command": "/old/bin/skiman sync --hook" }]
        }));
    write_json(&claude_settings(&sandbox), &settings);

    init(&sandbox, "claude-code").code(0);

    assert_eq!(
        read_json(&claude_settings(&sandbox)),
        with_skiman_group(user_settings())
    );
}

#[test]
fn session_hooks_for_a_tool_that_is_not_installed_writes_nothing_and_warns() {
    let sandbox = Sandbox::builder()
        .tool(Tool::ClaudeCode, ToolSetup::Absent)
        .build();

    init(&sandbox, "claude-code").code(0).stderr_eq(str![[r#"
warning: [..]/home/.claude does not exist, so skiman adds no session-start hook entry for claude-code

"#]]);

    assert!(!sandbox.presence_dir(Tool::ClaudeCode).exists());
}

#[test]
fn session_hooks_none_removes_only_skimans_entries() {
    let sandbox = Sandbox::new();
    let mut settings = with_skiman_group(user_settings());
    settings["hooks"]["SessionStart"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "type": "command", "command": "/old/bin/skiman sync --hook" }));
    write_json(&claude_settings(&sandbox), &settings);
    let codex = json!({
        "description": "mine",
        "hooks": {
            "SessionStart": [
                { "hooks": [{ "type": "command", "command": "boot.sh" }] },
                { "matcher": "startup|resume",
                  "hooks": [{ "type": "command", "command": "/usr/local/bin/skiman sync --hook" }] }
            ],
            "PreToolUse": [{ "hooks": [{ "type": "command", "command": "guard.sh" }] }]
        }
    });
    write_json(&codex_hooks(&sandbox), &codex);

    init(&sandbox, "none").code(0).stdout_eq(str![[r#"
created skill repo [..]/skill-repo
...
removed the session-start hook entries from [..]/home/.claude/settings.json
removed the session-start hook entries from [..]/home/.codex/hooks.json

"#]]);

    assert_eq!(read_json(&claude_settings(&sandbox)), user_settings());
    let mut codex_after = codex.clone();
    codex_after["hooks"]["SessionStart"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert_eq!(read_json(&codex_hooks(&sandbox)), codex_after);

    // Nothing left to remove: no rewrite, no output.
    let bytes = std::fs::read(claude_settings(&sandbox)).unwrap();
    init(&sandbox, "none").code(0).stdout_eq("");
    assert_eq!(std::fs::read(claude_settings(&sandbox)).unwrap(), bytes);
}

#[test]
fn session_hooks_none_after_adding_restores_the_settings_and_creates_no_codex_file() {
    let sandbox = Sandbox::new();
    init(&sandbox, "claude-code").code(0);
    write_json(
        &claude_settings(&sandbox),
        &with_skiman_group(user_settings()),
    );

    init(&sandbox, "none").code(0);
    assert_eq!(read_json(&claude_settings(&sandbox)), user_settings());

    // A file holding only skiman's entry ends as an empty object.
    std::fs::remove_file(claude_settings(&sandbox)).unwrap();
    init(&sandbox, "claude-code").code(0);
    init(&sandbox, "none").code(0);
    assert_eq!(read_json(&claude_settings(&sandbox)), json!({}));
    assert!(!codex_hooks(&sandbox).exists());
}

#[test]
fn the_entry_command_runs_when_the_binary_path_has_a_space() {
    let sandbox = Sandbox::new();
    let bin = sandbox.root().join("my bin");
    std::fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_skiman"), bin.join("skiman")).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    sandbox
        .command("skiman")
        .env("PATH", path)
        .args(["init", "--session-hooks", "claude-code"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);

    let settings = read_json(&claude_settings(&sandbox));
    let command = settings["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        command,
        format!("{}/my\\ bin/skiman sync --hook", sandbox.root().display())
    );
    sandbox
        .command("/bin/sh")
        .args(["-c", &command])
        .assert()
        .code(0)
        .stdout_eq("");
}

/// What `init` shows before asking, for the entry in `settings`.
fn preview(settings: &Path) -> String {
    format!(
        "skiman can add this session-start hook entry to `hooks.SessionStart` in {}, so a new or resumed session refreshes distribution:\n{}\n",
        settings.display(),
        serde_json::to_string_pretty(&skiman_group()).unwrap()
    )
}

fn init_pty(sandbox: &Sandbox) -> support::Pty {
    sandbox.skiman_pty([
        std::ffi::OsString::from("init"),
        sandbox.skill_repo().into_os_string(),
    ])
}

#[test]
fn with_a_terminal_init_previews_the_exact_entry_and_declining_changes_nothing() {
    let sandbox = Sandbox::new();
    write_json(&claude_settings(&sandbox), &user_settings());
    let before = std::fs::read(claude_settings(&sandbox)).unwrap();

    let mut pty = init_pty(&sandbox);
    let shown = pty.expect("Add this entry? [y/N] ").replace('\r', "");
    pty.send_line("n");
    let (code, rest) = pty.wait();

    assert_eq!(code, 0, "{rest}");
    assert!(
        shown.ends_with(&preview(&claude_settings(&sandbox))),
        "{shown}"
    );
    assert!(
        shown.contains(&format!(
            "warning: session refresh for Codex is not available until it is confirmed (Phase 1 experiment 4); skiman adds no entry to {}\n",
            codex_hooks(&sandbox).display()
        )),
        "{shown}"
    );
    assert_eq!(std::fs::read(claude_settings(&sandbox)).unwrap(), before);
    assert!(!codex_hooks(&sandbox).exists());
    assert!(rest.contains("--session-hooks claude-code"), "{rest}");
}

#[test]
fn with_a_terminal_accepting_the_preview_writes_the_entry() {
    let sandbox = Sandbox::new();
    write_json(&claude_settings(&sandbox), &user_settings());

    let mut pty = init_pty(&sandbox);
    pty.expect("Add this entry? [y/N] ");
    pty.send_line("y");
    let (code, rest) = pty.wait();

    assert_eq!(code, 0, "{rest}");
    assert!(
        rest.contains("added the session-start hook entry to"),
        "{rest}"
    );
    assert_eq!(
        read_json(&claude_settings(&sandbox)),
        with_skiman_group(user_settings())
    );
    assert!(!codex_hooks(&sandbox).exists());

    // Already present: no question the second time.
    let pty = init_pty(&sandbox);
    let (code, rest) = pty.wait();
    assert_eq!(code, 0, "{rest}");
    assert!(!rest.contains("[y/N]"), "{rest}");
}
