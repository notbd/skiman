//! `skiman sync [--hook]`: link distribution, `managing-skills` and the repo lock.

mod support;

use std::os::unix::fs::MetadataExt;
use std::time::{Duration, Instant};

use snapbox::str;
use support::{Sandbox, Tool, ToolSetup, assert_symlink, tree};

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn write_skill(dir: &std::path::Path, name: &str) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    std::fs::write(
        dir.join(name).join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\n---\n"),
    )
    .unwrap();
}

#[test]
fn sync_links_a_skill_folder_created_by_hand() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.skill_repo().join("skills"), "notes");

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
linked [..]/home/.claude/skills/notes
linked [..]/home/.agents/skills/notes
linked [..]/home/.gemini/config/skills/notes

"#]])
        .stderr_eq("");

    let target = sandbox.skill_repo().join("skills/notes");
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("notes"), &target, &target);
    }
}

#[test]
fn sync_links_only_distributable_skill_folders() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skills = sandbox.skill_repo().join("skills");
    write_skill(&skills, "Bad_Name");
    write_skill(&skills, "synced");
    std::fs::create_dir(skills.join("empty")).unwrap();
    std::fs::write(skills.join("loose-file"), "x").unwrap();

    sandbox.skiman().arg("sync").assert().code(0).stdout_eq("");

    for tool in Tool::ALL {
        let names: Vec<_> = tree(sandbox.tool_dir(tool)).into_keys().collect();
        assert_eq!(names, [std::path::PathBuf::from("managing-skills")]);
    }
}

#[test]
fn managing_skills_is_linked_everywhere_holds_the_repo_path_and_is_not_rewritten() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    let materialised = repo.join(".skiman/managing-skills");

    for tool in Tool::ALL {
        assert_symlink(
            sandbox.tool_dir(tool).join("managing-skills"),
            &materialised,
            &materialised,
        );
    }
    let skill_md = materialised.join("SKILL.md");
    let content = std::fs::read_to_string(&skill_md).unwrap();
    assert!(content.starts_with("---\nname: managing-skills\ndescription: "));
    assert!(
        content.contains(&format!("The skill repo is `{}`.", repo.display())),
        "{content}"
    );
    let inode = std::fs::metadata(&skill_md).unwrap().ino();

    sandbox.skiman().arg("sync").assert().code(0).stdout_eq("");

    assert_eq!(std::fs::metadata(&skill_md).unwrap().ino(), inode);
    assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), content);
}

#[test]
fn managing_skills_is_rewritten_when_its_content_differs() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skill_md = sandbox
        .skill_repo()
        .join(".skiman/managing-skills/SKILL.md");
    let content = std::fs::read_to_string(&skill_md).unwrap();
    std::fs::write(&skill_md, "edited").unwrap();

    sandbox.skiman().arg("sync").assert().code(0);

    assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), content);
}

#[test]
fn sync_skips_a_tool_whose_presence_directory_is_missing_and_creates_a_missing_tool_directory() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Codex, ToolSetup::PresenceOnly)
        .build();
    init(&sandbox);
    std::fs::remove_dir_all(sandbox.presence_dir(Tool::Antigravity)).unwrap();
    write_skill(&sandbox.skill_repo().join("skills"), "notes");

    sandbox.skiman().arg("sync").assert().code(0);

    let target = sandbox.skill_repo().join("skills/notes");
    assert_symlink(
        sandbox.tool_dir(Tool::Codex).join("notes"),
        &target,
        &target,
    );
    assert!(!sandbox.presence_dir(Tool::Antigravity).exists());
}

#[test]
fn sync_hook_with_nothing_to_change_prints_nothing_and_exits_0() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.skill_repo().join("skills"), "notes");
    sandbox.skiman().arg("sync").assert().code(0);
    let before: Vec<_> = Tool::ALL
        .iter()
        .map(|t| tree(sandbox.tool_dir(*t)))
        .collect();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");

    let after: Vec<_> = Tool::ALL
        .iter()
        .map(|t| tree(sandbox.tool_dir(*t)))
        .collect();
    assert_eq!(after, before);
}

#[test]
fn sync_hook_links_new_skills_without_printing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.skill_repo().join("skills"), "notes");

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");

    let target = sandbox.skill_repo().join("skills/notes");
    assert_symlink(
        sandbox.tool_dir(Tool::ClaudeCode).join("notes"),
        &target,
        &target,
    );
}

#[test]
fn while_another_process_holds_the_lock_sync_hook_exits_0_silently_and_new_fails_after_10_seconds()
{
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.skill_repo().join("skills"), "notes");
    let lock = std::fs::File::options()
        .write(true)
        .open(sandbox.skill_repo().join(".skiman/lock"))
        .unwrap();
    lock.lock().unwrap();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    assert!(
        std::fs::symlink_metadata(sandbox.tool_dir(Tool::ClaudeCode).join("notes")).is_err(),
        "a busy lock means sync --hook does nothing"
    );

    let started = Instant::now();
    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "d"])
        .assert()
        .code(4)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: another skiman command holds [..]/skill-repo/.skiman/lock (waited 10 seconds)

"#]]);
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(9_500) && waited < Duration::from_secs(20),
        "waited {waited:?}"
    );
    assert!(!sandbox.skill_repo().join("skills/pdf").exists());

    drop(lock);
    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "d"])
        .assert()
        .code(0);
}

#[test]
fn init_on_a_clone_on_a_second_machine_distributes_every_committed_skill() {
    let first = Sandbox::new();
    init(&first);
    for name in ["notes", "pdf"] {
        first
            .skiman()
            .args(["new", name, "-d", "d"])
            .assert()
            .code(0);
    }
    first.git(first.skill_repo(), ["add", "--all"]);
    first.git(
        first.skill_repo(),
        [
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.test",
            "commit",
            "--quiet",
            "-m",
            "skills",
        ],
    );

    let second = Sandbox::new();
    let clone = second.root().join("elsewhere/my-skills");
    second.git(
        second.root(),
        ["clone", "--quiet"]
            .map(std::ffi::OsString::from)
            .into_iter()
            .chain([first.skill_repo().into(), clone.clone().into()]),
    );

    second
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&clone)
        .assert()
        .code(0);

    for name in ["notes", "pdf"] {
        let target = clone.join("skills").join(name);
        for tool in Tool::ALL {
            assert_symlink(second.tool_dir(tool).join(name), &target, &target);
        }
    }
    let managing = clone.join(".skiman/managing-skills");
    assert_symlink(
        second.tool_dir(Tool::Codex).join("managing-skills"),
        &managing,
        &managing,
    );
}
