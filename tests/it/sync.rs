//! `skiman sync [--hook]`: link distribution, `managing-skills` and the repo lock.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use snapbox::str;

use crate::support::{
    Sandbox, Tool, ToolSetup, assert_symlink, exists, init, moved_note, tree, write,
};

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
fn sync_hook_does_not_write_managing_skills_through_a_linked_folder_and_names_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let materialised = sandbox.skill_repo().join(".skiman/managing-skills");
    std::fs::remove_dir_all(&materialised).unwrap();
    let foreign = sandbox.root().join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("SKILL.md"), "user original").unwrap();
    std::os::unix::fs::symlink(&foreign, &materialised).unwrap();

    let assert = sandbox.skiman().args(["sync", "--hook"]).assert();

    assert_eq!(
        std::fs::read_to_string(foreign.join("SKILL.md")).unwrap(),
        "user original",
        "sync wrote through the link"
    );
    assert_eq!(std::fs::read_link(&materialised).unwrap(), foreign);
    assert.code(0).stdout_eq("").stderr_eq(str![[r#"
error: cannot write [..]/skill-repo/.skiman/managing-skills: [..]/skill-repo/.skiman/managing-skills is not a real folder (a link or a file is in the way)

"#]]);
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

/// `sync`, with the test hook writing a regular file to `path` after sync
/// found `path` empty and right before it creates its link there.
fn sync_while_a_file_appears_at(sandbox: &Sandbox, path: &Path) -> snapbox::cmd::OutputAssert {
    sandbox
        .skiman()
        .arg("sync")
        .env("SKIMAN_TEST_HOOK", format!("sync:link:{}", path.display()))
        .env(
            "SKIMAN_TEST_HOOK_RUN",
            format!("printf mine > '{}'", path.display()),
        )
        .assert()
}

/// `path` is still the file the test hook wrote.
fn assert_kept(path: &Path) {
    let meta = std::fs::symlink_metadata(path).unwrap();
    assert!(meta.is_file(), "{} was replaced by a link", path.display());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "mine");
}

#[test]
fn a_file_that_appears_where_sync_is_about_to_link_a_skill_is_kept_and_reported_as_occupied() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.skill_repo().join("skills"), "notes");
    let path = sandbox.tool_dir(Tool::ClaudeCode).join("notes");

    let assert = sync_while_a_file_appears_at(&sandbox, &path);

    assert_kept(&path);
    assert
        .code(3)
        .stdout_eq(str![[r#"
linked [..]/home/.agents/skills/notes
linked [..]/home/.gemini/config/skills/notes

"#]])
        .stderr_eq(str![[r#"
warning: [..]/home/.claude/skills/notes is occupied by an entry skiman does not own, so skill notes is not linked there; move it away, then run `skiman sync`

"#]]);
}

#[test]
fn a_file_that_appears_where_sync_is_about_to_link_the_shared_body_is_kept_and_reported_as_refused()
{
    let sandbox = Sandbox::new();
    init(&sandbox);
    let path = sandbox.instruction_file(Tool::ClaudeCode);
    std::fs::remove_file(&path).unwrap();

    let assert = sync_while_a_file_appears_at(&sandbox, &path);

    assert_kept(&path);
    assert.code(3).stdout_eq("").stderr_eq(str![[r#"
warning: instruction file [..]/home/.claude/CLAUDE.md is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`

"#]]);
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

/// skiman notes a move to a backup in its state folder, never in an entry:
/// a skill holding a file named like the old note installs like any other,
/// and a note for a foreign skill that names no copy in the backups is
/// ignored.
#[test]
fn a_file_named_like_skimans_move_note_is_only_content() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let source = sandbox.root().join("src/bar");
    write_skill(source.parent().unwrap(), "bar");
    std::fs::write(source.join(".skiman-moved-to"), "/nonexistent/copy").unwrap();
    sandbox
        .skiman()
        .arg("add")
        .arg(&source)
        .arg("--yes")
        .assert()
        .code(0);
    assert!(
        sandbox
            .skill_repo()
            .join("skills/bar/.skiman-moved-to")
            .is_file()
    );
    assert!(sandbox.skill_repo().join("upstream/bar/SKILL.md").is_file());

    let tool_dir = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&tool_dir, "baz");
    let entry = tool_dir.join("baz");
    let meta = std::fs::symlink_metadata(&entry).unwrap();
    let note = moved_note(&sandbox.state_home(), &entry);
    write(note, format!("{} {}\n/elsewhere", meta.dev(), meta.ino()));
    let sync = sandbox.skiman().arg("sync").output().unwrap();
    let stderr = String::from_utf8_lossy(&sync.stderr);
    assert!(stderr.contains("foreign skill"), "{stderr}");
    let doctor = sandbox.skiman().arg("doctor").output().unwrap();
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(!stdout.contains("cut short"), "{stdout}");
}

/// A FIFO in what a move to another filesystem left is never read: the
/// session-start sync does not block on it (it would hold skiman's lock),
/// doctor does not say to delete what is left, and nothing is deleted.
#[test]
fn a_fifo_in_what_a_move_cut_short_left_does_not_block_sync_hook() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let tool_dir = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&tool_dir, "notes");
    let copy = sandbox
        .state_home()
        .join("skiman/backups/20200101T000000Z/claude-code/notes");
    write_skill(copy.parent().unwrap(), "notes");
    let entry = tool_dir.join("notes");
    let meta = std::fs::symlink_metadata(&entry).unwrap();
    let note = moved_note(&sandbox.state_home(), &entry);
    write(
        &note,
        format!("{} {}\n{}", meta.dev(), meta.ino(), copy.display()),
    );
    let made = sandbox
        .std_command("mkfifo")
        .arg(entry.join("pipe"))
        .status()
        .unwrap();
    assert!(made.success());

    let mut hook = sandbox
        .std_command(env!("CARGO_BIN_EXE_skiman"))
        .args(["sync", "--hook"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = hook.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(10) {
            hook.kill().unwrap();
            hook.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(
        status.and_then(|s| s.code()),
        Some(0),
        "sync --hook blocked"
    );
    assert!(exists(entry.join("pipe")) && entry.join("SKILL.md").is_file());
    assert!(note.is_file());
    // The copy holds no FIFO, so doctor does not say to delete the rest.
    let doctor = sandbox.skiman().arg("doctor").output().unwrap();
    let said = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        said.contains("changed since") && !said.contains("; delete it"),
        "{said}"
    );
}

/// One write at a time on a machine: a command that waited for the
/// machine lock while another switched the skill repo writes nothing.
#[test]
fn a_command_that_waited_while_the_skill_repo_was_switched_writes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let other = sandbox.root().join("other-repo");
    std::fs::create_dir(&other).unwrap();
    let config = sandbox.config_home().join("skiman/config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    let repo = sandbox.skill_repo().display().to_string();
    let switched = text.replace(&repo, &other.display().to_string());
    assert_ne!(switched, text);
    let lock = std::fs::File::options()
        .write(true)
        .open(sandbox.state_home().join("skiman/lock"))
        .unwrap();
    lock.lock().unwrap();

    let output = std::thread::scope(|scope| {
        let new = scope.spawn(|| sandbox.skiman().args(["new", "pdf", "-d", "d"]).output());
        std::thread::sleep(Duration::from_millis(500));
        std::fs::write(&config, &switched).unwrap();
        drop(lock);
        new.join().unwrap().unwrap()
    });

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "the skill repo changed to {}",
            other.canonicalize().unwrap().display()
        )),
        "{stderr}"
    );
    assert!(!sandbox.skill_repo().join("skills/pdf").exists());
}
