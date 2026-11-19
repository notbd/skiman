//! The project part of `skiman sync [--hook]`: one test per row of the
//! state table in `docs/project-scope.md` ("Sync: the state table"), plus the
//! pause while the canon is mid-operation, hook mode, and tool-directory
//! links into a folder a retired `skill_dirs` key named.

use std::path::{Path, PathBuf};

use crate::support::{Sandbox, Tool, backups, exists, init, read, skill_md, tree, write};

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The project root `work` at `~/work`; created, not registered.
fn root(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.home().join("work");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn register(sandbox: &Sandbox, dir: &Path) {
    sandbox
        .skiman()
        .args(["project", "add", "--id", "work"])
        .arg(dir)
        .assert()
        .code(0);
}

fn adopt_all(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
}

/// The mirror of `rel` under root `work`.
fn mirror(sandbox: &Sandbox, rel: &str) -> PathBuf {
    sandbox.canon().join("projects/work").join(rel)
}

fn synced_file(sandbox: &Sandbox) -> PathBuf {
    sandbox.state_home().join("skiman/projects-synced.toml")
}

fn backup_count(sandbox: &Sandbox) -> usize {
    std::fs::read_dir(backups(sandbox)).map_or(0, |dirs| dirs.count())
}

fn sync(sandbox: &Sandbox) -> std::process::Output {
    sandbox.skiman().arg("sync").output().unwrap()
}

fn hook(sandbox: &Sandbox) -> std::process::Output {
    sandbox.skiman().args(["sync", "--hook"]).output().unwrap()
}

/// A registered root with `proj/AGENTS.md` ("v1") adopted: sides equal,
/// synced hash recorded. Returns the root.
fn adopted_file(sandbox: &Sandbox) -> PathBuf {
    init(sandbox);
    let dir = root(sandbox);
    write(dir.join("proj/AGENTS.md"), "v1\n");
    register(sandbox, &dir);
    adopt_all(sandbox);
    assert_eq!(read(mirror(sandbox, "proj/AGENTS.md")), "v1\n");
    dir
}

/// Asserts what `rel`'s project file and its mirror hold (`None`: absent).
#[track_caller]
fn assert_sides(
    sandbox: &Sandbox,
    dir: &Path,
    rel: &str,
    disk: Option<&str>,
    mirrored: Option<&str>,
) {
    let read_opt = |p: PathBuf| exists(&p).then(|| read(p));
    assert_eq!(
        read_opt(dir.join(rel)).as_deref(),
        disk,
        "project side of {rel}"
    );
    assert_eq!(
        read_opt(mirror(sandbox, rel)).as_deref(),
        mirrored,
        "mirror side of {rel}"
    );
}

// ------------------------------------------------------------ row 12

#[test]
fn an_item_in_step_on_both_sides_syncs_silently() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "");
    assert_eq!(stderr_of(&out), "");
    assert_sides(&sandbox, &dir, "proj/AGENTS.md", Some("v1\n"), Some("v1\n"));
    assert_eq!(backup_count(&sandbox), 0);
}

// ------------------------------------------------------------ row 13

#[test]
fn a_project_edit_is_copied_into_the_mirror_with_the_old_mirror_in_a_backup() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    write(dir.join("proj/AGENTS.md"), "v2\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert_eq!(
        stdout_of(&out),
        format!("mirrored {}\n", dir.join("proj/AGENTS.md").display())
    );
    assert_eq!(stderr_of(&out), "");
    assert_sides(&sandbox, &dir, "proj/AGENTS.md", Some("v2\n"), Some("v2\n"));
    let backups: Vec<PathBuf> = std::fs::read_dir(backups(&sandbox))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1, "one backup folder: {backups:?}");
    assert_eq!(
        read(backups[0].join("repo/projects/work/proj/AGENTS.md")),
        "v1\n",
        "the replaced mirror is in the backup"
    );
    assert_eq!(
        tree(sandbox.canon().join(".skiman/swap")).len(),
        0,
        "no swap folder is left behind"
    );

    // The synced hash moved to v2: the next sync has nothing to do.
    let again = sync(&sandbox);
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(stdout_of(&again), "");
    assert_eq!(stderr_of(&again), "");
}

/// A mirror staging folder that cannot be opened stops every mirror write
/// of the run with one error, not one per item.
#[test]
fn a_mirror_staging_that_cannot_be_opened_is_reported_once() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    write(dir.join("other/AGENTS.md"), "other v1\n");
    adopt_all(&sandbox);
    write(dir.join("proj/AGENTS.md"), "v2\n");
    write(dir.join("other/AGENTS.md"), "other v2\n");
    let swap = sandbox.canon().join(".skiman/swap");
    std::fs::create_dir_all(&swap).unwrap();
    std::fs::set_permissions(&swap, std::fs::Permissions::from_mode(0o000)).unwrap();

    let out = sync(&sandbox);
    std::fs::set_permissions(&swap, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(out.status.code(), Some(4), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    let said = format!("cannot create a folder in {}", swap.display());
    assert_eq!(stderr.matches(&said).count(), 1, "{stderr}");
    assert_sides(&sandbox, &dir, "proj/AGENTS.md", Some("v2\n"), Some("v1\n"));
}

/// A tmp leftover of the mirror that cannot be moved to the backup stays
/// where it is, and the project edits are mirrored all the same.
#[test]
fn a_mirror_tmp_leftover_that_cannot_be_moved_does_not_stop_sync() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    write(dir.join("proj/AGENTS.md"), "v2\n");
    let leftover = sandbox.canon().join(".skiman/swap/20200101T000000Z");
    write(leftover.join("old/projects/work/proj/AGENTS.md"), "older\n");
    // A folder moves to another parent only with write access to it.
    let mode = |m| std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(m)).unwrap();
    mode(0o555);
    let out = sync(&sandbox);
    mode(0o755);

    assert_sides(&sandbox, &dir, "proj/AGENTS.md", Some("v2\n"), Some("v2\n"));
    let said = format!(
        "cannot move the tmp leftover {} to a backup: ",
        leftover.display()
    );
    assert!(
        stderr_of(&out).contains(&said) || stdout_of(&out).contains(&said),
        "{out:?}"
    );
    assert_eq!(
        read(leftover.join("old/projects/work/proj/AGENTS.md")),
        "older\n"
    );
}

#[test]
fn an_edited_project_skill_folder_is_copied_into_the_mirror() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    let skill = dir.join("proj/.claude/skills/x");
    write(skill.join("SKILL.md"), skill_md("x"));
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    write(skill.join("notes/extra.md"), "more\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), format!("mirrored {}\n", skill.display()));
    let mirrored = mirror(&sandbox, "proj/.claude/skills/x");
    assert_eq!(read(mirrored.join("notes/extra.md")), "more\n");
    assert_eq!(read(mirrored.join("SKILL.md")), skill_md("x"));
}

#[test]
fn a_project_edit_with_a_secret_is_blocked_and_the_mirror_kept() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    let secret = format!("key = {}{}\n", "AKIA", "ABCDEFGHIJKLMNOP");
    write(dir.join("proj/AGENTS.md"), &secret);

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_of(&out), "", "nothing is mirrored");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{}: it is not copied into the mirror",
            dir.join("proj/AGENTS.md").display()
        )),
        "{stderr}"
    );
    assert_sides(
        &sandbox,
        &dir,
        "proj/AGENTS.md",
        Some(&secret),
        Some("v1\n"),
    );
    assert_eq!(backup_count(&sandbox), 0);
    // The synced hash still names v1, so the block is reported again.
    assert_eq!(sync(&sandbox).status.code(), Some(3));
}

#[test]
fn a_project_file_over_the_size_limit_is_blocked_and_the_mirror_kept() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    let file = std::fs::File::create(dir.join("proj/AGENTS.md")).unwrap();
    file.set_len(10 * 1024 * 1024 + 1).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_of(&out), "");
    let stderr = stderr_of(&out);
    assert!(stderr.contains("is not copied into the mirror"), "{stderr}");
    assert!(stderr.contains("over the limit"), "{stderr}");
    assert_eq!(read(mirror(&sandbox, "proj/AGENTS.md")), "v1\n");
}

// ------------------------------------------------------------ row 14

#[test]
fn a_changed_mirror_is_reported_and_the_project_left_alone() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    write(mirror(&sandbox, "proj/AGENTS.md"), "from git\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_of(&out), "");
    let item = dir.join("proj/AGENTS.md");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{}: its mirror changed; write it to the project: skiman project restore {}; or keep the project's version: skiman project adopt {}",
            item.display(),
            item.display(),
            item.display()
        )),
        "{stderr}"
    );
    assert_sides(
        &sandbox,
        &dir,
        "proj/AGENTS.md",
        Some("v1\n"),
        Some("from git\n"),
    );
    assert_eq!(backup_count(&sandbox), 0);
}

// ------------------------------------------------------------ row 15

#[test]
fn both_sides_changed_is_a_conflict_and_neither_is_written() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    write(dir.join("proj/AGENTS.md"), "project\n");
    write(mirror(&sandbox, "proj/AGENTS.md"), "mirror\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_of(&out), "");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{}: it and its mirror both changed",
            dir.join("proj/AGENTS.md").display()
        )),
        "{stderr}"
    );
    assert_sides(
        &sandbox,
        &dir,
        "proj/AGENTS.md",
        Some("project\n"),
        Some("mirror\n"),
    );
}

#[test]
fn differing_sides_with_nothing_recorded_are_a_conflict() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    std::fs::remove_file(synced_file(&sandbox)).unwrap();
    write(dir.join("proj/AGENTS.md"), "project\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert!(
        stderr_of(&out).contains("both changed"),
        "{}",
        stderr_of(&out)
    );
    assert_sides(
        &sandbox,
        &dir,
        "proj/AGENTS.md",
        Some("project\n"),
        Some("v1\n"),
    );
}

#[test]
fn several_items_needing_a_decision_end_with_one_command_naming_all_of_them() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    write(dir.join("a/AGENTS.md"), "a\n");
    write(dir.join("b/AGENTS.md"), "b\n");
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    write(mirror(&sandbox, "a/AGENTS.md"), "a2\n");
    write(dir.join("b/AGENTS.md"), "b-project\n");
    write(mirror(&sandbox, "b/AGENTS.md"), "b-mirror\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    let (a, b) = (dir.join("a/AGENTS.md"), dir.join("b/AGENTS.md"));
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "skiman project adopt {} {}; or the mirror's: skiman project restore {} {}",
            a.display(),
            b.display(),
            a.display(),
            b.display()
        )),
        "{stderr}"
    );
}

// ------------------------------------------------------------ row 10

#[test]
fn an_item_deleted_in_the_project_is_reported_and_not_brought_back() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    std::fs::remove_file(dir.join("proj/AGENTS.md")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    let item = dir.join("proj/AGENTS.md");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{}: it was deleted in the project; bring it back from the mirror: skiman project restore {}",
            item.display(),
            item.display()
        )),
        "{stderr}"
    );
    assert_sides(&sandbox, &dir, "proj/AGENTS.md", None, Some("v1\n"));
}

// ------------------------------------------------------------ row 11

#[test]
fn an_item_deleted_in_the_mirror_is_reported_and_not_copied_back() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    std::fs::remove_file(mirror(&sandbox, "proj/AGENTS.md")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_of(&out), "");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{d}: it was deleted in the mirror; take it back in: skiman project adopt {d}",
            d = dir.join("proj/AGENTS.md").display()
        )),
        "{stderr}"
    );
    assert_sides(&sandbox, &dir, "proj/AGENTS.md", Some("v1\n"), None);
}

// ------------------------------------------------------------ row 4

#[test]
fn an_item_gone_from_both_sides_has_its_synced_hash_dropped() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    assert!(read(synced_file(&sandbox)).contains("proj/AGENTS.md"));
    std::fs::remove_file(dir.join("proj/AGENTS.md")).unwrap();
    std::fs::remove_file(mirror(&sandbox, "proj/AGENTS.md")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "");
    assert_eq!(stderr_of(&out), "");
    assert!(
        !read(synced_file(&sandbox)).contains("proj/AGENTS.md"),
        "{}",
        read(synced_file(&sandbox))
    );
    // A file created there later is a new candidate, not "deleted in the mirror".
    write(dir.join("proj/AGENTS.md"), "fresh\n");
    let later = sync(&sandbox);
    assert_eq!(later.status.code(), Some(0), "{}", stderr_of(&later));
    assert_eq!(stderr_of(&later), "");
    assert!(!exists(mirror(&sandbox, "proj/AGENTS.md")));
}

// ------------------------------------------------------------ row 6

#[test]
fn a_project_folder_that_is_gone_is_reported_once_and_does_not_need_attention() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    write(dir.join("proj/AGENTS.md"), "a\n");
    write(dir.join("proj/.claude/skills/x/SKILL.md"), skill_md("x"));
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    std::fs::remove_dir_all(dir.join("proj")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(0));
    let stderr = stderr_of(&out);
    let line = format!(
        "its project folder is gone; if it moved, adopt the new path; to stop managing it: skiman project eject {}",
        dir.join("proj").display()
    );
    assert_eq!(stderr.matches(&line).count(), 1, "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(exists(mirror(&sandbox, "proj/AGENTS.md")));
    assert!(exists(mirror(&sandbox, "proj/.claude/skills/x/SKILL.md")));
    assert!(!exists(dir.join("proj")), "sync never writes the project");
}

// ------------------------------------------------------------ rows 7 and 9

#[test]
fn mirror_items_this_machine_never_had_get_one_line_per_root_with_a_restore_command() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    register(&sandbox, &dir);
    // Another machine adopted these; here only `a` and `b` exist, empty.
    write(mirror(&sandbox, "a/AGENTS.md"), "a\n");
    write(
        mirror(&sandbox, "b/.claude/skills/x/SKILL.md"),
        skill_md("x"),
    );
    write(mirror(&sandbox, "c/AGENTS.md"), "c\n");
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "");
    let stderr = stderr_of(&out);
    let expected = format!(
        "warning: 2 project item(s) in the mirror are not on this machine yet; write them with: skiman project restore {}\n",
        dir.display()
    );
    // `c` has no project folder here (row 7): silent, not counted.
    assert_eq!(stderr, expected);
    assert!(
        !exists(dir.join("a/AGENTS.md")),
        "sync never writes the project"
    );

    // The command is paste-ready: run as printed, it writes both items.
    let command = stderr
        .trim_end()
        .rsplit_once("write them with: ")
        .unwrap()
        .1;
    let words: Vec<&str> = command.split(' ').collect();
    assert_eq!(&words[..3], ["skiman", "project", "restore"]);
    sandbox
        .skiman()
        .args(&words[1..])
        .arg("--yes")
        .assert()
        .code(0);
    assert_eq!(read(dir.join("a/AGENTS.md")), "a\n");
    assert_eq!(read(dir.join("b/.claude/skills/x/SKILL.md")), skill_md("x"));
    assert!(!exists(dir.join("c")));
}

// ------------------------------------------------------------ row 8

#[test]
fn a_symlinked_folder_on_the_way_to_an_item_is_occupied_and_not_followed() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    write(dir.join("proj/.claude/skills/x/SKILL.md"), skill_md("x"));
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    // `.claude` becomes a link to a folder elsewhere holding a changed skill.
    let elsewhere = sandbox.root().join("elsewhere/.claude");
    write(elsewhere.join("skills/x/SKILL.md"), "changed elsewhere\n");
    std::fs::remove_dir_all(dir.join("proj/.claude")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join("proj/.claude")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_of(&out), "", "nothing is mirrored through the link");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{} is not a real folder",
            dir.join("proj/.claude").display()
        )),
        "{stderr}"
    );
    assert_eq!(
        read(mirror(&sandbox, "proj/.claude/skills/x/SKILL.md")),
        skill_md("x")
    );
    assert_eq!(
        read(elsewhere.join("skills/x/SKILL.md")),
        "changed elsewhere\n"
    );
}

#[test]
fn a_symlink_at_the_item_path_is_occupied_and_not_followed() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    let item = dir.join("proj/AGENTS.md");
    write(dir.join("proj/other.md"), "other\n");
    std::fs::remove_file(&item).unwrap();
    std::os::unix::fs::symlink("other.md", &item).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_of(&out), "");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{}: {} is not a regular file",
            item.display(),
            item.display()
        )),
        "{stderr}"
    );
    assert_eq!(std::fs::read_link(&item).unwrap(), Path::new("other.md"));
    assert_eq!(read(mirror(&sandbox, "proj/AGENTS.md")), "v1\n");
}

// ------------------------------------------------------------ row 5

#[test]
fn a_mirror_skill_holding_a_symlink_is_invalid_and_the_project_left_alone() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    let skill = dir.join("proj/.claude/skills/x");
    write(skill.join("SKILL.md"), skill_md("x"));
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    std::os::unix::fs::symlink(
        "SKILL.md",
        mirror(&sandbox, "proj/.claude/skills/x/alias.md"),
    )
    .unwrap();
    write(skill.join("SKILL.md"), "edited in the project\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_of(&out), "", "an invalid mirror is not replaced");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("fix projects/work/proj/.claude/skills/x in the canon"),
        "{stderr}"
    );
    assert_eq!(read(skill.join("SKILL.md")), "edited in the project\n");
    assert!(!exists(skill.join("alias.md")));
    assert_eq!(
        read(mirror(&sandbox, "proj/.claude/skills/x/SKILL.md")),
        skill_md("x")
    );
}

#[test]
fn a_mirror_file_that_is_a_symlink_is_invalid() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    let mirrored = mirror(&sandbox, "proj/AGENTS.md");
    std::fs::remove_file(&mirrored).unwrap();
    std::os::unix::fs::symlink(dir.join("proj/AGENTS.md"), &mirrored).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert!(
        stderr_of(&out).contains("fix projects/work/proj/AGENTS.md in the canon"),
        "{}",
        stderr_of(&out)
    );
    assert!(std::fs::symlink_metadata(&mirrored).unwrap().is_symlink());
}

// ------------------------------------------------------------ row 2

#[test]
fn a_root_folder_missing_on_this_machine_gets_one_warning() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    write(dir.join("a/AGENTS.md"), "a\n");
    write(dir.join("b/AGENTS.md"), "b\n");
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    std::fs::remove_dir_all(&dir).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "");
    let stderr = stderr_of(&out);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.starts_with(&format!(
            "warning: project root `work` is skipped: {} is not on this machine",
            dir.display()
        )),
        "{stderr}"
    );
    assert_eq!(read(mirror(&sandbox, "a/AGENTS.md")), "a\n");
    assert!(
        read(synced_file(&sandbox)).contains("a/AGENTS.md"),
        "records are kept"
    );
}

// ------------------------------------------------------------ row 1

/// Puts the canon into a git state with `enter`, edits the adopted
/// project file, and asserts sync pauses the project part without writing
/// the mirror. (The edit comes second: `enter` may run git, whose hooks sync.)
#[track_caller]
fn assert_paused(enter: impl FnOnce(&Sandbox, &Path), phrase: &str) {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    enter(&sandbox, &sandbox.canon().join(".git"));
    write(dir.join("proj/AGENTS.md"), "v2\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "", "nothing is mirrored");
    let stderr = stderr_of(&out);
    assert_eq!(
        stderr,
        format!(
            "warning: the canon {phrase}, so project items are not synced; finish that, then run `skiman sync`\n"
        )
    );
    assert_sides(&sandbox, &dir, "proj/AGENTS.md", Some("v2\n"), Some("v1\n"));
    assert_eq!(backup_count(&sandbox), 0);
}

#[test]
fn project_sync_pauses_while_the_canon_is_mid_merge() {
    assert_paused(
        |_, git_dir| write(git_dir.join("MERGE_HEAD"), "0".repeat(40)),
        "is in the middle of a merge",
    );
}

#[test]
fn project_sync_pauses_while_the_canon_is_mid_rebase() {
    assert_paused(
        |_, git_dir| std::fs::create_dir(git_dir.join("rebase-merge")).unwrap(),
        "is in the middle of a rebase",
    );
}

#[test]
fn project_sync_pauses_while_the_canon_has_a_detached_head() {
    assert_paused(
        |sandbox, _| {
            let repo = sandbox.canon();
            sandbox.git(&repo, ["add", "-A"]);
            sandbox.git(
                &repo,
                [
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.com",
                    "commit",
                    "--no-verify",
                    "-qm",
                    "base",
                ],
            );
            sandbox.git(&repo, ["checkout", "-q", "--detach"]);
        },
        "has a detached HEAD (a `git checkout` of a commit, or a bisect)",
    );
}

// ------------------------------------------------------------ sync --hook

#[test]
fn sync_hook_prints_each_project_report_once_and_exits_0() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    write(mirror(&sandbox, "proj/AGENTS.md"), "from git\n");

    let first = hook(&sandbox);
    assert_eq!(
        first.status.code(),
        Some(0),
        "exit 0 even though it needs attention"
    );
    let stdout = stdout_of(&first);
    let line = format!(
        "skiman: {}: its mirror changed",
        dir.join("proj/AGENTS.md").display()
    );
    assert_eq!(stdout.matches(&line).count(), 1, "{stdout}");
    assert_eq!(stdout.lines().count(), 1, "{stdout}");

    let second = hook(&sandbox);
    assert_eq!(second.status.code(), Some(0));
    assert_eq!(
        stdout_of(&second),
        "",
        "an already reported state is not repeated"
    );
    assert_sides(
        &sandbox,
        &dir,
        "proj/AGENTS.md",
        Some("v1\n"),
        Some("from git\n"),
    );

    // A plain sync still reports it: the recorded set only quiets the hook.
    assert_eq!(sync(&sandbox).status.code(), Some(3));
}

#[test]
fn sync_hook_mirrors_a_project_edit_without_printing() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    write(dir.join("proj/AGENTS.md"), "v2\n");

    let out = hook(&sandbox);

    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout_of(&out), "");
    assert_eq!(stderr_of(&out), "");
    assert_eq!(read(mirror(&sandbox, "proj/AGENTS.md")), "v2\n");
}

// ------------------------------------------------------------ retired skill_dirs

/// A link into a folder a retired `skill_dirs` key named is foreign like any
/// other (ADR 0013); sync no longer reads the key.
#[test]
fn a_tool_directory_link_into_a_former_skill_dirs_folder_is_foreign() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    register(&sandbox, &dir);
    let manifest = sandbox.canon().join("skiman.toml");
    let text = read(&manifest).replace(
        "[projects.work]\n",
        "[projects.work]\nskill_dirs = [\"core/skills\"]\n",
    );
    assert!(text.contains("skill_dirs"), "{text}");
    std::fs::write(&manifest, text).unwrap();
    write(dir.join("core/skills/x/SKILL.md"), skill_md("x"));
    let tool_dir = sandbox.tool_dir(Tool::ClaudeCode);
    std::os::unix::fs::symlink(dir.join("core/skills/x"), tool_dir.join("x")).unwrap();

    let out = sync(&sandbox);

    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "foreign skill {} is not managed by skiman",
            tool_dir.join("x").display()
        )),
        "{stderr}"
    );
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(
        std::fs::read_link(tool_dir.join("x")).unwrap(),
        dir.join("core/skills/x"),
        "the link is left as it is"
    );
}

// ------------------------------------------------------------ more rows

#[test]
fn a_missing_folder_between_the_anchor_and_the_item_means_deleted_not_occupied() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    write(dir.join("proj/AGENTS.md"), "a\n");
    write(dir.join("proj/.claude/skills/x/SKILL.md"), skill_md("x"));
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    std::fs::remove_dir_all(dir.join("proj/.claude")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{}: it was deleted in the project",
            dir.join("proj/.claude/skills/x").display()
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("not synced"), "not occupied: {stderr}");
}

#[test]
fn a_project_skill_whose_skill_md_was_renamed_away_is_blocked() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    let skill = dir.join("proj/.claude/skills/x");
    write(skill.join("SKILL.md"), skill_md("x"));
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    std::fs::rename(skill.join("SKILL.md"), skill.join("SKILL.md.bak")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{}: it is not copied into the mirror",
            skill.display()
        )),
        "{stderr}"
    );
    let mirrored = mirror(&sandbox, "proj/.claude/skills/x");
    assert_eq!(read(mirrored.join("SKILL.md")), skill_md("x"));
    assert!(!exists(mirrored.join("SKILL.md.bak")));
}

#[test]
fn a_mirror_skill_without_skill_md_is_invalid() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    let skill = dir.join("proj/.claude/skills/x");
    write(skill.join("SKILL.md"), skill_md("x"));
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    std::fs::remove_file(mirror(&sandbox, "proj/.claude/skills/x/SKILL.md")).unwrap();
    write(mirror(&sandbox, "proj/.claude/skills/x/other.md"), "o\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert!(
        stderr_of(&out).contains("fix projects/work/proj/.claude/skills/x in the canon"),
        "{}",
        stderr_of(&out)
    );
    assert_eq!(read(skill.join("SKILL.md")), skill_md("x"));
    assert!(!exists(skill.join("other.md")));
}

/// The state a canon has after its retired `skill_dirs` key is removed
/// (ADR 0013): a mirror skill and a synced hash at a path with no item shape.
#[test]
fn a_mirror_path_that_lost_its_item_shape_is_skipped_and_its_synced_hash_dropped() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    write(dir.join("proj/AGENTS.md"), "a\n");
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    write(mirror(&sandbox, "core/skills/x/SKILL.md"), skill_md("x"));
    write(dir.join("core/skills/x/SKILL.md"), "edited\n");
    let recorded = read(synced_file(&sandbox));
    assert!(recorded.contains("\"proj/AGENTS.md\""), "{recorded}");
    let planted = recorded.replace(
        "[work.items]\n",
        &format!(
            "[work.items]\n\"core/skills/x\" = \"skill sha256:{}\"\n",
            "0".repeat(64)
        ),
    );
    assert!(planted.contains("core/skills/x"), "{recorded}");
    std::fs::write(synced_file(&sandbox), planted).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "");
    assert_eq!(stderr_of(&out), "", "`check` reports the path, not sync");
    let synced = read(synced_file(&sandbox));
    assert!(!synced.contains("core/skills/x"), "{synced}");
    assert!(
        synced.contains("\"proj/AGENTS.md\""),
        "the root's record stays: {synced}"
    );
    assert_eq!(
        read(mirror(&sandbox, "core/skills/x/SKILL.md")),
        skill_md("x")
    );
    assert_eq!(read(dir.join("core/skills/x/SKILL.md")), "edited\n");
}

#[test]
fn synced_hashes_recorded_for_another_root_path_are_not_trusted() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let old = root(&sandbox);
    write(old.join("a/AGENTS.md"), "a\n");
    write(old.join("b/AGENTS.md"), "b\n");
    register(&sandbox, &old);
    adopt_all(&sandbox);
    // The root moved, and `path` was changed (say, on another machine).
    let new = sandbox.home().join("work2");
    std::fs::rename(&old, &new).unwrap();
    let manifest = sandbox.canon().join("skiman.toml");
    let text = read(&manifest).replace("path = \"~/work\"", "path = \"~/work2\"");
    std::fs::write(&manifest, text).unwrap();
    write(new.join("b/AGENTS.md"), "b edited\n");

    let out = sync(&sandbox);

    // `a` is equal on both sides and recorded again; `b` differs with
    // nothing recorded for this path, so it is a conflict, not row 13.
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_of(&out), "", "nothing is mirrored");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "{}: it and its mirror both changed",
            new.join("b/AGENTS.md").display()
        )),
        "{stderr}"
    );
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert_eq!(read(mirror(&sandbox, "b/AGENTS.md")), "b\n");
    let synced = read(synced_file(&sandbox));
    assert!(synced.contains(&new.display().to_string()), "{synced}");
    assert!(synced.contains("a/AGENTS.md"), "{synced}");
}

// ------------------------------------------------------------ product bugs

#[test]
fn items_deleted_in_the_mirror_are_not_offered_a_restore() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = root(&sandbox);
    write(dir.join("a/AGENTS.md"), "a\n");
    write(dir.join("b/AGENTS.md"), "b\n");
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    std::fs::remove_file(mirror(&sandbox, "a/AGENTS.md")).unwrap();
    std::fs::remove_file(mirror(&sandbox, "b/AGENTS.md")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    let stderr = stderr_of(&out);
    // Row 11's fixes are `adopt <item>` or `eject`; there is no mirror to restore.
    assert!(!stderr.contains("skiman project restore"), "{stderr}");
}

#[test]
fn the_fix_command_for_one_item_is_paste_ready_for_a_path_with_a_space() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = sandbox.home().join("my work");
    write(dir.join("a/AGENTS.md"), "a\n");
    register(&sandbox, &dir);
    adopt_all(&sandbox);
    std::fs::remove_file(dir.join("a/AGENTS.md")).unwrap();

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains(&format!(
            "skiman project restore '{}'",
            dir.join("a/AGENTS.md").display()
        )),
        "{stderr}"
    );
}

#[test]
fn a_symlinked_folder_in_the_mirror_is_never_written_through() {
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    // Move the mirror folder out of the repo and leave a link in its place.
    let outside = sandbox.root().join("outside");
    std::fs::rename(mirror(&sandbox, "proj"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, mirror(&sandbox, "proj")).unwrap();
    write(dir.join("proj/AGENTS.md"), "v2\n");

    let out = sync(&sandbox);

    assert_eq!(out.status.code(), Some(3));
    assert!(
        stderr_of(&out).contains("its mirror cannot be used"),
        "{}",
        stderr_of(&out)
    );
    assert_eq!(
        read(outside.join("AGENTS.md")),
        "v1\n",
        "nothing written outside the repo"
    );
}

#[test]
fn a_backup_that_cannot_be_written_still_records_what_was_mirrored() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    let dir = adopted_file(&sandbox);
    std::fs::create_dir_all(backups(&sandbox)).unwrap();
    let locked = std::fs::Permissions::from_mode(0o500);
    std::fs::set_permissions(backups(&sandbox), locked).unwrap();
    write(dir.join("proj/AGENTS.md"), "v2\n");

    let out = sync(&sandbox);
    std::fs::set_permissions(backups(&sandbox), std::fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(out.status.code(), Some(4), "{}", stderr_of(&out));
    assert!(
        stderr_of(&out).contains("it is kept at"),
        "{}",
        stderr_of(&out)
    );
    assert_eq!(read(mirror(&sandbox, "proj/AGENTS.md")), "v2\n");
    // The next project edit is an ordinary change, not a conflict.
    write(dir.join("proj/AGENTS.md"), "v3\n");
    let out = sync(&sandbox);
    assert!(
        !stderr_of(&out).contains("both changed"),
        "{}",
        stderr_of(&out)
    );
    assert_eq!(read(mirror(&sandbox, "proj/AGENTS.md")), "v3\n");
}
