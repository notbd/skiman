//! `skiman adopt <name> --from <tool id>`: copy a foreign skill into the
//! skill repo, move the original entry to a backup, link the copy.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use snapbox::str;

use crate::support::{Sandbox, Tool, assert_symlink, tree};

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn exists(path: impl AsRef<Path>) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn is_executable(path: impl AsRef<Path>) -> bool {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o100 != 0
}

/// The whole skill repo except `.git`, every tool directory and the
/// backups, to prove a command changed nothing.
fn world(sandbox: &Sandbox) -> Vec<std::collections::BTreeMap<PathBuf, String>> {
    let repo = sandbox.skill_repo();
    let mut world = vec![
        tree(repo.join("skills")),
        tree(repo.join("upstream")),
        tree(repo.join(".skiman/tmp")),
        tree(sandbox.state_home().join("skiman/backups")),
        [repo.join("skiman.toml"), repo.join("skiman.lock")]
            .iter()
            .map(|f| (f.clone(), std::fs::read_to_string(f).unwrap_or_default()))
            .collect(),
    ];
    world.extend(Tool::ALL.iter().map(|t| tree(sandbox.tool_dir(*t))));
    world
}

/// The only folder under `backups/`.
fn only_backup(sandbox: &Sandbox) -> PathBuf {
    let backups = sandbox.state_home().join("skiman/backups");
    let dirs: Vec<PathBuf> = std::fs::read_dir(&backups)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "one backup folder: {dirs:?}");
    dirs.into_iter().next().unwrap()
}

fn check_is_clean(sandbox: &Sandbox) {
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

fn assert_linked(sandbox: &Sandbox, name: &str) {
    let target = sandbox.skill_repo().join("skills").join(name);
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join(name), &target, &target);
    }
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Does {name} things\n---\n\n# {name}\n")
}

/// A foreign skill folder `notes` in Claude Code's tool directory, made
/// after `init`, so it is foreign.
fn foreign_notes(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
    // Another name in the frontmatter: adopt rewrites the name line.
    write(dir.join("SKILL.md"), skill_md("my-notes"));
    write(dir.join("scripts/run.sh"), "#!/bin/sh\necho run\n");
    std::fs::set_permissions(
        dir.join("scripts/run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    write(dir.join(".DS_Store"), "finder");
    write(dir.join(".git/HEAD"), "ref: refs/heads/main\n");
    dir
}

#[test]
fn adopt_of_a_foreign_folder_copies_it_backs_up_the_original_and_links_the_copy() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let original = foreign_notes(&sandbox);
    let original_tree = tree(&original);

    sandbox
        .skiman()
        .args(["adopt", "notes", "--from", "claude-code", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
created [..]/skill-repo/skills/notes
moved [..]/home/.claude/skills/notes to [..]/state/skiman/backups/[..]/claude-code/notes
linked [..]/home/.claude/skills/notes
linked [..]/home/.agents/skills/notes
linked [..]/home/.gemini/config/skills/notes

"#]])
        .stderr_eq("");

    let copy = sandbox.skill_repo().join("skills/notes");
    assert_eq!(
        read(copy.join("SKILL.md")),
        skill_md("my-notes").replace("name: my-notes", "name: notes"),
        "only the name line is rewritten"
    );
    assert!(is_executable(copy.join("scripts/run.sh")));
    assert!(
        !exists(copy.join(".DS_Store")),
        "never copied into the skill repo"
    );
    assert!(
        !exists(copy.join(".git")),
        "a .git folder is not skill content"
    );
    // The original, `.DS_Store` and `.git` included, is in the backup.
    let backup = only_backup(&sandbox);
    assert_eq!(tree(backup.join("claude-code/notes")), original_tree);
    assert_linked(&sandbox, "notes");
    check_is_clean(&sandbox);
}

#[test]
fn adopt_of_a_foreign_symlink_moves_the_link_as_a_link_and_never_touches_its_target() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = sandbox.home().join("elsewhere/ext-skill");
    write(target.join("SKILL.md"), skill_md("ext"));
    write(target.join("reference.md"), "# Reference\n");
    let link = sandbox.tool_dir(Tool::Codex).join("ext");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let target_before = tree(&target);

    sandbox
        .skiman()
        .args(["adopt", "ext", "--from", "codex", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
created [..]/skill-repo/skills/ext
moved [..]/home/.agents/skills/ext to [..]/state/skiman/backups/[..]/codex/ext
linked [..]/home/.claude/skills/ext
linked [..]/home/.agents/skills/ext
linked [..]/home/.gemini/config/skills/ext

"#]])
        .stderr_eq("");

    let backup = only_backup(&sandbox);
    assert_eq!(
        std::fs::read_link(backup.join("codex/ext")).unwrap(),
        target,
        "the backup holds the link itself"
    );
    assert_eq!(tree(&target), target_before, "the link target is untouched");
    assert_eq!(
        read(sandbox.skill_repo().join("skills/ext/reference.md")),
        "# Reference\n"
    );
    assert_linked(&sandbox, "ext");
    check_is_clean(&sandbox);
}

#[test]
fn adopt_refuses_every_name_conflict_and_changes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    for name in [
        "dangling",
        "tabled",
        "locked",
        "snapped",
        "Bad_Name",
        "managing-skills",
        "synced",
    ] {
        write(
            sandbox
                .tool_dir(Tool::ClaudeCode)
                .join(name)
                .join("SKILL.md"),
            skill_md(name),
        );
    }
    std::fs::create_dir_all(repo.join("skills")).unwrap();
    std::os::unix::fs::symlink("/nowhere", repo.join("skills/dangling")).unwrap();
    let mut manifest = read(repo.join("skiman.toml"));
    manifest.push_str("\n[skills.tabled]\nenabled = false\n");
    std::fs::write(repo.join("skiman.toml"), manifest).unwrap();
    write(
        repo.join("skiman.lock"),
        format!(
            "version = 1\n\n[skills.locked]\nsnapshot = \"sha256:{}\"\n",
            "0".repeat(64)
        ),
    );
    write(repo.join("upstream/snapped/SKILL.md"), skill_md("snapped"));
    let before = world(&sandbox);

    for (name, message) in [
        ("dangling", "skills/dangling already exists"),
        ("tabled", "skiman.toml already has a [skills.tabled] table"),
        ("locked", "skiman.lock already has an entry for locked"),
        ("snapped", "upstream/snapped already exists"),
        ("Bad_Name", "is not a valid skill name"),
        ("managing-skills", "is a reserved skill name"),
        ("synced", "is a reserved skill name"),
    ] {
        let assert = sandbox
            .skiman()
            .args(["adopt", name, "--from", "claude-code", "--yes"])
            .assert()
            .code(1)
            .stdout_eq("");
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains(message), "{name}: {stderr}");
        assert!(
            stderr.contains("rename or move the foreign entry") && stderr.contains("by hand"),
            "{name}: {stderr}"
        );
        assert_eq!(world(&sandbox), before, "{name} changed something");
    }
}

#[test]
fn adopt_refuses_a_foreign_skill_with_a_symlink_inside_and_changes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
    write(dir.join("SKILL.md"), skill_md("notes"));
    std::os::unix::fs::symlink("/etc/hosts", dir.join("hosts")).unwrap();
    let before = world(&sandbox);

    let assert = sandbox
        .skiman()
        .args(["adopt", "notes", "--from", "claude-code", "--yes"])
        .assert()
        .code(1)
        .stdout_eq("");
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("hosts"), "{stderr}");
    assert!(stderr.contains("symlink"), "{stderr}");
    assert_eq!(world(&sandbox), before);
}

#[test]
fn adopt_refuses_a_foreign_skill_that_breaks_a_rule_and_leaves_no_copy() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
    let long = "x".repeat(1025);
    write(
        dir.join("SKILL.md"),
        format!("---\nname: notes\ndescription: {long}\n---\n"),
    );
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["adopt", "notes", "--from", "claude-code", "--yes"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: refusing to write content that breaks the rules:
  skills/notes/SKILL.md:3: description-length: [..]

"#]]);
    assert_eq!(world(&sandbox), before);
}

#[test]
fn files_inside_a_dot_git_folder_do_not_count_toward_the_file_limit() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
    write(dir.join("SKILL.md"), skill_md("notes"));
    write(dir.join("reference.md"), "# Reference\n");
    write(dir.join("scripts/run.sh"), "echo run\n");
    for i in 0..2000 {
        write(dir.join(format!(".git/objects/{:02x}/{i}", i % 256)), "x");
    }

    sandbox
        .skiman()
        .args(["adopt", "notes", "--from", "claude-code", "--yes"])
        .assert()
        .code(0);

    let copy = sandbox.skill_repo().join("skills/notes");
    assert!(copy.join("reference.md").is_file());
    assert!(copy.join("scripts/run.sh").is_file());
    assert!(!exists(copy.join(".git")));
}

#[test]
fn adopt_refuses_a_foreign_skill_over_the_file_limit_and_changes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
    write(dir.join("SKILL.md"), skill_md("notes"));
    for i in 0..1000 {
        write(dir.join(format!("many/{i}.md")), "x");
    }
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["adopt", "notes", "--from", "claude-code", "--yes"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: [..]/home/.claude/skills/notes holds more than 1000 files; a skill may hold at most 1000

"#]]);
    assert_eq!(world(&sandbox), before);
}

#[test]
fn adopt_whose_original_cannot_be_moved_exits_4_and_prints_the_manual_finish() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let original = foreign_notes(&sandbox);
    let tool_dir = sandbox.tool_dir(Tool::ClaudeCode);
    std::fs::set_permissions(&tool_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let output = sandbox
        .skiman()
        .args(["adopt", "notes", "--from", "claude-code", "--yes"])
        .output()
        .unwrap();
    std::fs::set_permissions(&tool_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let backup = only_backup(&sandbox);
    let dest = backup.join("claude-code/notes");
    assert!(
        stderr.contains(&format!(
            "finish by hand: move {} to {}, then run `skiman sync`",
            original.display(),
            dest.display()
        )),
        "{stderr}"
    );
    assert!(sandbox.skill_repo().join("skills/notes/SKILL.md").is_file());
    assert!(original.join("SKILL.md").is_file(), "the original stays");

    // The manual finish works.
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::rename(&original, &dest).unwrap();
    sandbox.skiman().arg("sync").assert().code(0);
    assert_linked(&sandbox, "notes");
    check_is_clean(&sandbox);
}

#[test]
fn without_a_terminal_adopt_needs_yes() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    foreign_notes(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["adopt", "notes", "--from", "claude-code"])
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: missing flag without a terminal: --yes

"#]]);
    assert_eq!(world(&sandbox), before);
}

#[test]
fn with_a_terminal_adopt_shows_the_files_and_asks() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    foreign_notes(&sandbox);
    let before = world(&sandbox);

    let mut pty = sandbox.skiman_pty(["adopt", "notes", "--from", "claude-code"]);
    let shown = pty.expect("Adopt skill notes? [y/N] ");
    assert!(shown.contains("scripts/run.sh"), "{shown}");
    pty.send_line("n");
    let (code, rest) = pty.wait();
    assert_eq!(code, 1, "{rest}");
    assert!(rest.contains("not adopted; nothing was changed"), "{rest}");
    assert_eq!(world(&sandbox), before);

    let mut pty = sandbox.skiman_pty(["adopt", "notes", "--from", "claude-code"]);
    pty.expect("Adopt skill notes? [y/N] ");
    pty.send_line("y");
    let (code, rest) = pty.wait();
    assert_eq!(code, 0, "{rest}");
    assert_linked(&sandbox, "notes");
}

#[test]
fn adopt_refuses_an_entry_that_is_missing_owned_or_not_a_skill() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "mine", "-d", "Mine"])
        .assert()
        .code(0);
    write(
        sandbox.tool_dir(Tool::ClaudeCode).join("plain/README.md"),
        "no SKILL.md here\n",
    );
    let before = world(&sandbox);

    for (name, message) in [
        ("absent", "has no entry"),
        ("plain", "is not a skill folder"),
    ] {
        let assert = sandbox
            .skiman()
            .args(["adopt", name, "--from", "claude-code", "--yes"])
            .assert()
            .code(1)
            .stdout_eq("");
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains(message), "{name}: {stderr}");
    }
    // An owned link is refused as taken: skills/mine exists.
    sandbox
        .skiman()
        .args(["adopt", "mine", "--from", "claude-code", "--yes"])
        .assert()
        .code(1);
    sandbox
        .skiman()
        .args(["adopt", "notes", "--from", "cursor", "--yes"])
        .assert()
        .code(2);
    assert_eq!(world(&sandbox), before);
}
