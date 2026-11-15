//! `skiman new <name> -d <description>`: create an own skill and distribute it.

use snapbox::str;

use crate::support::{Sandbox, Tool, assert_each_kill_finishes, assert_symlink, init, tree};

/// The skill repo and every tool directory, to prove a command changed nothing.
fn world(sandbox: &Sandbox) -> Vec<std::collections::BTreeMap<std::path::PathBuf, String>> {
    let mut world = vec![
        tree(sandbox.skill_repo().join("skills")),
        tree(sandbox.skill_repo().join("upstream")),
    ];
    world.extend(Tool::ALL.iter().map(|t| tree(sandbox.tool_dir(*t))));
    world
}

#[test]
fn new_creates_a_valid_skill_and_links_it_into_every_installed_tool_directory() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    // Codex is enabled but no longer present; Antigravity's tool directory is missing.
    std::fs::remove_dir_all(sandbox.presence_dir(Tool::Codex)).unwrap();
    std::fs::remove_dir_all(sandbox.tool_dir(Tool::Antigravity)).unwrap();
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "Fill PDF forms"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
created [..]/skill-repo/skills/pdf
linked [..]/home/.claude/skills/pdf
linked [..]/home/.gemini/config/skills/pdf
linked [..]/home/.gemini/config/skills/managing-skills

"#]])
        .stderr_eq("");

    assert_eq!(
        std::fs::read_to_string(repo.join("skills/pdf/SKILL.md")).unwrap(),
        "---\nname: pdf\ndescription: \"Fill PDF forms\"\n---\n\n# pdf\n"
    );
    let target = repo.join("skills/pdf");
    for tool in [Tool::ClaudeCode, Tool::Antigravity] {
        assert_symlink(sandbox.tool_dir(tool).join("pdf"), &target, &target);
    }
    assert!(
        std::fs::symlink_metadata(sandbox.tool_dir(Tool::Codex).join("pdf")).is_err(),
        "codex is no longer present, so it gets nothing"
    );
    let no_commits = sandbox
        .std_command("git")
        .current_dir(&repo)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .status()
        .unwrap();
    assert!(!no_commits.success(), "new must not commit");
}

#[test]
fn an_edit_in_the_skill_repo_is_visible_through_every_link_at_once() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "Fill PDF forms"])
        .assert()
        .code(0);

    let edited = "---\nname: pdf\ndescription: Fill and sign PDF forms\n---\n\nUse pdftk.\n";
    std::fs::write(sandbox.skill_repo().join("skills/pdf/SKILL.md"), edited).unwrap();

    for tool in Tool::ALL {
        let through_link = sandbox.tool_dir(tool).join("pdf/SKILL.md");
        assert_eq!(std::fs::read_to_string(through_link).unwrap(), edited);
    }
}

#[test]
fn new_also_links_a_skill_folder_created_by_hand() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    std::fs::create_dir(repo.join("skills/notes")).unwrap();
    std::fs::write(
        repo.join("skills/notes/SKILL.md"),
        "---\nname: notes\ndescription: Take notes\n---\n",
    )
    .unwrap();

    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "Fill PDF forms"])
        .assert()
        .code(0);

    let target = repo.join("skills/notes");
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("notes"), &target, &target);
    }
}

#[test]
fn new_writes_the_description_as_valid_yaml_whatever_it_contains() {
    let sandbox = Sandbox::new();
    init(&sandbox);

    sandbox
        .skiman()
        .args([
            "new",
            "quote",
            "-d",
            "Say \"hi\": a\\b\nnext # not a comment",
        ])
        .assert()
        .code(0);

    assert_eq!(
        std::fs::read_to_string(sandbox.skill_repo().join("skills/quote/SKILL.md")).unwrap(),
        "---\nname: quote\ndescription: \"Say \\\"hi\\\": a\\\\b\\nnext # not a comment\"\n---\n\n# quote\n"
    );
}

#[test]
fn new_refuses_invalid_and_reserved_names_and_writes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let before = world(&sandbox);
    let too_long = "a".repeat(65);

    for name in ["PDF", "pdf_forms", "a--b", "pdf-", "-", too_long.as_str()] {
        sandbox
            .skiman()
            .args(["new", "-d", "d", "--", name])
            .assert()
            .code(1)
            .stdout_eq("")
            .stderr_eq(format!(
                "error: `{name}` is not a valid skill name: use 1 to 64 lowercase letters, digits and single hyphens between them (^[a-z0-9]+(-[a-z0-9]+)*$)\n"
            ));
    }
    for name in ["managing-skills", "synced", ".hidden"] {
        sandbox
            .skiman()
            .args(["new", "-d", "d", "--", name])
            .assert()
            .code(1)
            .stdout_eq("")
            .stderr_eq(format!(
                "error: `{name}` is a reserved skill name (reserved: managing-skills, synced, and any name starting with `.`)\n"
            ));
    }
    assert_eq!(world(&sandbox), before);

    sandbox
        .skiman()
        .args(["new", "a".repeat(64).as_str(), "-d", "d"])
        .assert()
        .code(0);
}

#[test]
fn new_refuses_each_form_of_taken_name_and_writes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    std::fs::write(repo.join("skills/file"), "not a folder").unwrap();
    std::os::unix::fs::symlink(repo.join("nowhere"), repo.join("skills/dangling")).unwrap();
    std::fs::create_dir(repo.join("skills/no-skill-md")).unwrap();
    std::fs::write(repo.join("skills/no-skill-md/README.md"), "no SKILL.md").unwrap();
    std::fs::create_dir(repo.join("skills/empty-group")).unwrap();
    std::fs::write(
        repo.join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n\n[skills.parked]\nenabled = false\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("skiman.lock"),
        "version = 1\n[skills.locked]\nsnapshot = \"sha256:00\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(repo.join("upstream/snap")).unwrap();
    let before = world(&sandbox);

    for (name, reason) in [
        ("file", "skills/file already exists"),
        ("dangling", "skills/dangling already exists"),
        ("no-skill-md", "skills/no-skill-md already exists"),
        ("empty-group", "skills/empty-group is a group folder"),
        ("parked", "skiman.toml already has a [skills.parked] table"),
        ("locked", "skiman.lock already has an entry for locked"),
        ("snap", "upstream/snap already exists"),
    ] {
        sandbox
            .skiman()
            .args(["new", name, "-d", "d"])
            .assert()
            .code(1)
            .stdout_eq("")
            .stderr_eq(format!("error: the name `{name}` is taken: {reason}\n"));
    }
    assert_eq!(world(&sandbox), before);
}

#[test]
fn new_refuses_an_empty_or_too_long_description() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["new", "pdf", "-d", ""])
        .assert()
        .code(1)
        .stderr_eq("error: the description must be 1 to 1024 characters (it has 0)\n");
    sandbox
        .skiman()
        .args(["new", "pdf", "-d"])
        .arg("é".repeat(1025))
        .assert()
        .code(1)
        .stderr_eq("error: the description must be 1 to 1024 characters (it has 1025)\n");
    assert_eq!(world(&sandbox), before);

    sandbox
        .skiman()
        .args(["new", "pdf", "-d"])
        .arg("é".repeat(1024))
        .assert()
        .code(0);
}

#[test]
fn new_without_a_terminal_requires_the_description_flag() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["new", "pdf"])
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq("error: missing flag without a terminal: -d <description>\n");

    assert_eq!(world(&sandbox), before);
}

#[test]
fn new_in_a_terminal_asks_for_the_description() {
    let sandbox = Sandbox::new();
    init(&sandbox);

    let mut pty = sandbox.skiman_pty(["new", "pdf"]);
    pty.expect("Description: ");
    pty.send_line("Fill PDF forms");
    let (code, _) = pty.wait();

    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(sandbox.skill_repo().join("skills/pdf/SKILL.md")).unwrap(),
        "---\nname: pdf\ndescription: \"Fill PDF forms\"\n---\n\n# pdf\n"
    );
}

#[test]
fn new_before_init_fails_and_names_init() {
    let sandbox = Sandbox::new();

    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "d"])
        .assert()
        .code(4)
        .stderr_eq(str![[r#"
error: [..]/home/.config/skiman/config.toml does not exist; run `skiman init <path>` first

"#]]);
}

/// Staging never writes outside the skill repo: a `.skiman/tmp` or a
/// `skills/` that is a link stops `new` before anything is written there.
#[test]
fn new_writes_nothing_through_a_linked_tmp_or_skills_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    // What a link leads to is not skiman's to move either: no tmp leftover.
    std::fs::write(elsewhere.join("theirs.md"), "the user's\n").unwrap();
    for (linked, name) in [(".skiman/tmp", "foo"), ("skills", "bar")] {
        let path = repo.join(linked);
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();

        let output = sandbox
            .skiman()
            .args(["new", name, "-d", "Use when testing."])
            .output()
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(4), "{linked}: {stderr}");
        assert!(
            stderr.contains("is not a real folder"),
            "{linked}: {stderr}"
        );
        assert_eq!(
            tree(&elsewhere).into_keys().collect::<Vec<_>>(),
            [std::path::PathBuf::from("theirs.md")],
            "{linked}"
        );
    }
}

/// Killed after the skill folder is in place, before the sync, `new`
/// finishes when run again: the folder it wrote does not take the name.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_new_killed_at_any_step_finishes_when_run_again() {
    assert_each_kill_finishes(
        |sandbox| {
            init(sandbox);
            ["new", "pdf", "-d", "Fill PDF forms"]
                .map(String::from)
                .to_vec()
        },
        &[
            "new:write:skills/pdf",
            "sync:link:<root>/home/.claude/skills/pdf",
        ],
    );
}
