//! `skiman project restore` and `skiman project eject` (plan "Commands",
//! "Writing: one protocol for every replace"). Restore under a folder writes
//! only what this machine never had; named exactly it also replaces, with
//! the project's version in `backups/*/projects/<id>/...`. Eject moves the
//! mirror to `backups/*/repo/projects/...` and never touches the project.

mod support;

use std::path::{Path, PathBuf};

use snapbox::cmd::Command;
use snapbox::str;
use support::{Sandbox, tree};

const AGENTS: &str = "# a\n\nUse pnpm.\n";

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

fn exists(path: impl AsRef<Path>) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Does {name} things\n---\n\n# {name}\n")
}

/// An AWS-shaped key, built at runtime so no literal token is committed.
fn secret() -> String {
    format!("AKIA{}", "ABCDEFGHIJKLMNOP")
}

/// The project root `~/work`, registered as `[projects.work]`.
fn root(sandbox: &Sandbox) -> PathBuf {
    sandbox.home().join("work")
}

/// `<root>/<rel>` on disk.
fn disk(sandbox: &Sandbox, rel: &str) -> PathBuf {
    root(sandbox).join(rel)
}

/// `projects/work/<rel>` in the skill repo.
fn mirror(sandbox: &Sandbox, rel: &str) -> PathBuf {
    sandbox.skill_repo().join("projects/work").join(rel)
}

fn synced_file(sandbox: &Sandbox) -> PathBuf {
    sandbox.state_home().join("skiman/projects-synced.toml")
}

fn backups_dir(sandbox: &Sandbox) -> PathBuf {
    sandbox.state_home().join("skiman/backups")
}

/// A skill repo with `~/work` registered; `files` are written under the
/// root first (`(rel, contents)`).
fn with_root(files: &[(&str, &str)]) -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    std::fs::create_dir_all(root(&sandbox)).unwrap();
    for (rel, contents) in files {
        write(disk(&sandbox, rel), contents);
    }
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(root(&sandbox))
        .assert()
        .code(0);
    sandbox
}

/// [`with_root`], then every file adopted into the mirror.
fn adopted(files: &[(&str, &str)]) -> Sandbox {
    let sandbox = with_root(files);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    for (rel, contents) in files {
        assert_eq!(&read(mirror(&sandbox, rel)), contents, "adopted {rel}");
    }
    sandbox
}

fn restore(sandbox: &Sandbox, paths: &[PathBuf]) -> Command {
    sandbox
        .skiman()
        .args(["project", "restore", "--yes"])
        .args(paths)
}

fn eject(sandbox: &Sandbox, paths: &[PathBuf]) -> Command {
    sandbox
        .skiman()
        .args(["project", "eject", "--yes"])
        .args(paths)
}

/// Every path `backups/<ts>/<rel>` that exists.
fn in_backups(sandbox: &Sandbox, rel: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(backups_dir(sandbox)) else {
        return Vec::new();
    };
    entries
        .map(|e| e.unwrap().path().join(rel))
        .filter(|p| exists(p))
        .collect()
}

/// The one backup copy of `rel`.
fn backed_up(sandbox: &Sandbox, rel: &str) -> PathBuf {
    let found = in_backups(sandbox, rel);
    assert_eq!(found.len(), 1, "one backup of {rel}: {found:?}");
    found.into_iter().next().unwrap()
}

/// No `projects/` folder in any backup: no project content was replaced.
fn assert_no_project_backup(sandbox: &Sandbox) {
    assert_eq!(in_backups(sandbox, "projects"), Vec::<PathBuf>::new());
}

/// `.skiman-swap-*` folders directly in `anchor`.
fn swap_folders(anchor: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(anchor)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".skiman-swap-")
        })
        .collect()
}

fn synced_text(sandbox: &Sandbox) -> String {
    std::fs::read_to_string(synced_file(sandbox)).unwrap_or_default()
}

fn sync_is_quiet(sandbox: &Sandbox) {
    sandbox.skiman().arg("sync").assert().code(0);
}

// ---------------------------------------------------------------- restore under a folder

#[test]
fn folder_restore_writes_the_items_this_machine_never_had() {
    let sandbox = with_root(&[("a/AGENTS.md", AGENTS)]);
    // Mirror items from another machine: a file and a skill whose
    // `.claude/skills` folder does not exist here yet.
    write(mirror(&sandbox, "a/AGENTS.md"), AGENTS);
    write(mirror(&sandbox, "a/CLAUDE.md"), "# claude\n");
    write(
        mirror(&sandbox, "a/.claude/skills/x/SKILL.md"),
        skill_md("x"),
    );
    write(mirror(&sandbox, "a/.claude/skills/x/notes.md"), "notes\n");

    restore(&sandbox, &[root(&sandbox)])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
write [..]/home/work/a/.claude/skills/x
write [..]/home/work/a/CLAUDE.md
restored [..]/home/work/a/.claude/skills/x
restored [..]/home/work/a/CLAUDE.md
...
"#]]);

    assert_eq!(read(disk(&sandbox, "a/CLAUDE.md")), "# claude\n");
    assert_eq!(
        tree(disk(&sandbox, "a/.claude/skills/x")),
        tree(mirror(&sandbox, "a/.claude/skills/x"))
    );
    assert_eq!(
        read(disk(&sandbox, "a/AGENTS.md")),
        AGENTS,
        "an equal item is left alone"
    );
    assert_no_project_backup(&sandbox);
    assert_eq!(swap_folders(&disk(&sandbox, "a")), Vec::<PathBuf>::new());
    let synced = synced_text(&sandbox);
    assert!(synced.contains("\"a/CLAUDE.md\""), "{synced}");
    assert!(synced.contains("\"a/.claude/skills/x\""), "{synced}");
    sync_is_quiet(&sandbox);
}

#[test]
fn folder_restore_never_replaces_content_nor_undoes_a_deletion() {
    let sandbox = adopted(&[
        ("deleted/AGENTS.md", AGENTS),
        ("mirror-changed/AGENTS.md", AGENTS),
        ("conflict/AGENTS.md", AGENTS),
    ]);
    std::fs::remove_file(disk(&sandbox, "deleted/AGENTS.md")).unwrap();
    write(mirror(&sandbox, "mirror-changed/AGENTS.md"), "mirror\n");
    write(mirror(&sandbox, "conflict/AGENTS.md"), "mirror\n");
    write(disk(&sandbox, "conflict/AGENTS.md"), "project\n");
    let before = tree(root(&sandbox));

    // Both the root and a folder inside it.
    restore(&sandbox, &[root(&sandbox), disk(&sandbox, "conflict")])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
nothing to restore

"#]]);

    assert_eq!(tree(root(&sandbox)), before, "the project is untouched");
    assert_no_project_backup(&sandbox);
}

// ---------------------------------------------------------------- restore named exactly

#[test]
fn exact_restore_brings_back_an_item_deleted_in_the_project() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    std::fs::remove_file(disk(&sandbox, "a/AGENTS.md")).unwrap();

    restore(&sandbox, &[disk(&sandbox, "a/AGENTS.md")])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
write the mirror's version over [..]/home/work/a/AGENTS.md
restored [..]/home/work/a/AGENTS.md
...
"#]]);

    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), AGENTS);
    assert_no_project_backup(&sandbox);
    assert_eq!(swap_folders(&disk(&sandbox, "a")), Vec::<PathBuf>::new());
    sync_is_quiet(&sandbox);
}

#[test]
fn exact_restore_of_a_mirror_change_backs_up_the_project_version() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    write(mirror(&sandbox, "a/AGENTS.md"), "from the mirror\n");

    restore(&sandbox, &[disk(&sandbox, "a/AGENTS.md")])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
write the mirror's version over [..]/home/work/a/AGENTS.md
restored [..]/home/work/a/AGENTS.md
replaced versions are in [..]/home/.local/state/skiman/backups/[..]
...
"#]]);

    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), "from the mirror\n");
    assert_eq!(
        read(backed_up(&sandbox, "projects/work/a/AGENTS.md")),
        AGENTS
    );
    assert_eq!(swap_folders(&disk(&sandbox, "a")), Vec::<PathBuf>::new());
    sync_is_quiet(&sandbox);
}

#[test]
fn exact_restore_of_a_conflict_takes_the_mirror_and_backs_up_the_project() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    write(mirror(&sandbox, "a/AGENTS.md"), "from the mirror\n");
    write(disk(&sandbox, "a/AGENTS.md"), "from the project\n");

    restore(&sandbox, &[disk(&sandbox, "a/AGENTS.md")])
        .assert()
        .code(0);

    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), "from the mirror\n");
    assert_eq!(
        read(backed_up(&sandbox, "projects/work/a/AGENTS.md")),
        "from the project\n"
    );
    assert_eq!(swap_folders(&disk(&sandbox, "a")), Vec::<PathBuf>::new());
    sync_is_quiet(&sandbox);
}

#[test]
fn exact_restore_of_a_skill_blocked_by_a_missing_skill_md_brings_back_the_mirror() {
    let sandbox = adopted(&[("a/.claude/skills/x/SKILL.md", &skill_md("x"))]);
    let skill = disk(&sandbox, "a/.claude/skills/x");
    std::fs::rename(skill.join("SKILL.md"), skill.join("README.md")).unwrap();
    // Sync keeps the mirror and reports the item blocked.
    sandbox.skiman().arg("sync").assert().code(3);
    assert_eq!(
        read(mirror(&sandbox, "a/.claude/skills/x/SKILL.md")),
        skill_md("x")
    );

    restore(&sandbox, std::slice::from_ref(&skill))
        .assert()
        .code(0);

    assert_eq!(read(skill.join("SKILL.md")), skill_md("x"));
    assert!(!exists(skill.join("README.md")));
    let old = backed_up(&sandbox, "projects/work/a/.claude/skills/x");
    assert_eq!(read(old.join("README.md")), skill_md("x"));
    assert_eq!(swap_folders(&disk(&sandbox, "a")), Vec::<PathBuf>::new());
    sync_is_quiet(&sandbox);
}

#[test]
fn exact_restore_of_a_file_blocked_by_a_secret_brings_back_the_mirror() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    let leaked = format!("{AGENTS}key = {}\n", secret());
    write(disk(&sandbox, "a/AGENTS.md"), &leaked);
    sandbox.skiman().arg("sync").assert().code(3);
    assert_eq!(read(mirror(&sandbox, "a/AGENTS.md")), AGENTS, "mirror kept");

    restore(&sandbox, &[disk(&sandbox, "a/AGENTS.md")])
        .assert()
        .code(0);

    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), AGENTS);
    assert_eq!(
        read(backed_up(&sandbox, "projects/work/a/AGENTS.md")),
        leaked
    );
}

#[test]
fn restore_does_not_write_a_path_a_project_git_repo_now_tracks() {
    let sandbox = adopted(&[("g/AGENTS.md", AGENTS)]);
    let repo = disk(&sandbox, "g");
    // The project's own repo starts tracking the file, which is then
    // deleted from its working tree.
    sandbox.git(&repo, ["init", "-q", "-b", "main"]);
    sandbox.git(&repo, ["add", "AGENTS.md"]);
    sandbox.git(&repo, ["commit", "-q", "-m", "track"]);
    std::fs::remove_file(repo.join("AGENTS.md")).unwrap();

    restore(&sandbox, &[repo.join("AGENTS.md")])
        .assert()
        .code(1)
        .stdout_eq(str![[r#"
nothing to restore

"#]])
        .stderr_eq(str![[r#"
skip 1 item(s) tracked by the git repo [..]/home/work/g (that repo already keeps their history):
  [..]/home/work/g/AGENTS.md
...
"#]]);

    assert!(!exists(repo.join("AGENTS.md")));
    assert_eq!(swap_folders(&repo), Vec::<PathBuf>::new());
}

#[test]
fn restore_moves_a_leftover_swap_folder_in_the_anchor_to_a_backup() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    write(mirror(&sandbox, "a/AGENTS.md"), "from the mirror\n");
    // What an interrupted restore leaves: the set-aside copy in `old/`.
    let leftover = disk(&sandbox, "a/.skiman-swap-20200101T000000Z");
    write(leftover.join(".gitignore"), "*\n");
    write(
        leftover.join("target"),
        disk(&sandbox, "a/AGENTS.md").display().to_string(),
    );
    write(leftover.join("old/AGENTS.md"), "only copy\n");
    std::fs::create_dir_all(leftover.join("new")).unwrap();

    restore(&sandbox, &[disk(&sandbox, "a/AGENTS.md")])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
...
moved the previous version of [..]/home/work/a/AGENTS.md, left by an interrupted write in [..]/home/work/a/.skiman-swap-20200101T000000Z, to [..]/home/.local/state/skiman/backups/[..]/swap-leftovers/.skiman-swap-20200101T000000Z
...
"#]]);

    assert!(!exists(&leftover));
    assert_eq!(
        read(backed_up(
            &sandbox,
            "swap-leftovers/.skiman-swap-20200101T000000Z/AGENTS.md"
        )),
        "only copy\n"
    );
    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), "from the mirror\n");
    assert_eq!(swap_folders(&disk(&sandbox, "a")), Vec::<PathBuf>::new());
}

#[test]
fn restore_dry_run_prints_the_plan_and_writes_nothing() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    write(mirror(&sandbox, "a/AGENTS.md"), "from the mirror\n");
    write(mirror(&sandbox, "b/AGENTS.md"), AGENTS);
    std::fs::create_dir_all(disk(&sandbox, "b")).unwrap();
    let world = || {
        (
            tree(root(&sandbox)),
            tree(backups_dir(&sandbox)),
            synced_text(&sandbox),
        )
    };
    let before = world();

    sandbox
        .skiman()
        .args(["project", "restore", "--dry-run"])
        .arg(disk(&sandbox, "a/AGENTS.md"))
        .arg(root(&sandbox))
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
write the mirror's version over [..]/home/work/a/AGENTS.md
write [..]/home/work/b/AGENTS.md

"#]]);

    assert_eq!(world(), before);
}

// ---------------------------------------------------------------- eject

#[test]
fn eject_of_an_item_moves_its_mirror_to_a_backup_and_leaves_the_project() {
    let sandbox = adopted(&[("a/sub/AGENTS.md", AGENTS), ("b/AGENTS.md", AGENTS)]);

    eject(&sandbox, &[disk(&sandbox, "a/sub/AGENTS.md")])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
stop managing projects/work/a/sub/AGENTS.md (its mirror goes to a backup; the project file stays)
moved projects/work/a/sub/AGENTS.md to [..]/home/.local/state/skiman/backups/[..]/repo/projects/work/a/sub/AGENTS.md
...
"#]]);

    assert_eq!(
        read(backed_up(&sandbox, "repo/projects/work/a/sub/AGENTS.md")),
        AGENTS
    );
    assert_eq!(read(disk(&sandbox, "a/sub/AGENTS.md")), AGENTS, "untouched");
    assert!(
        !exists(mirror(&sandbox, "a")),
        "empty mirror folders are removed"
    );
    assert_eq!(read(mirror(&sandbox, "b/AGENTS.md")), AGENTS);
    let synced = synced_text(&sandbox);
    assert!(!synced.contains("a/sub/AGENTS.md"), "{synced}");
    assert!(synced.contains("\"b/AGENTS.md\""), "{synced}");
    sync_is_quiet(&sandbox);
}

#[test]
fn eject_of_a_folder_ejects_every_item_under_it() {
    let sandbox = adopted(&[
        ("a/AGENTS.md", AGENTS),
        ("a/.claude/skills/x/SKILL.md", &skill_md("x")),
        ("b/AGENTS.md", AGENTS),
    ]);
    let project_before = tree(root(&sandbox));

    eject(&sandbox, &[disk(&sandbox, "a")]).assert().code(0);

    assert!(!exists(mirror(&sandbox, "a")));
    assert_eq!(
        read(backed_up(&sandbox, "repo/projects/work/a/AGENTS.md")),
        AGENTS
    );
    assert_eq!(
        read(backed_up(
            &sandbox,
            "repo/projects/work/a/.claude/skills/x/SKILL.md"
        )),
        skill_md("x")
    );
    assert_eq!(read(mirror(&sandbox, "b/AGENTS.md")), AGENTS);
    assert_eq!(tree(root(&sandbox)), project_before);
    let synced = synced_text(&sandbox);
    assert!(!synced.contains("\"a/"), "{synced}");
    assert!(synced.contains("\"b/AGENTS.md\""), "{synced}");
}

#[test]
fn eject_of_the_root_path_unregisters_the_root() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    let project_before = tree(root(&sandbox));

    eject(&sandbox, &[root(&sandbox)])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
...
removed [projects.work] from skiman.toml
...
"#]]);

    let manifest = read(sandbox.skill_repo().join("skiman.toml"));
    assert!(!manifest.contains("projects"), "{manifest}");
    assert!(!exists(sandbox.skill_repo().join("projects")));
    assert_eq!(
        read(backed_up(&sandbox, "repo/projects/work/a/AGENTS.md")),
        AGENTS
    );
    assert_eq!(tree(root(&sandbox)), project_before);
    assert!(!synced_text(&sandbox).contains("[work"));
    sandbox.skiman().arg("check").assert().code(0);
}

#[test]
fn eject_of_a_root_whose_folder_is_gone_still_unregisters_it() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    std::fs::remove_dir_all(root(&sandbox)).unwrap();

    eject(&sandbox, &[root(&sandbox)]).assert().code(0);

    let manifest = read(sandbox.skill_repo().join("skiman.toml"));
    assert!(!manifest.contains("projects"), "{manifest}");
    assert!(!exists(sandbox.skill_repo().join("projects")));
    assert_eq!(
        read(backed_up(&sandbox, "repo/projects/work/a/AGENTS.md")),
        AGENTS
    );
    assert!(!exists(root(&sandbox)), "nothing is created on disk");
    sandbox.skiman().arg("check").assert().code(0);
}

#[test]
fn eject_of_an_item_whose_project_folder_is_gone() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS), ("b/AGENTS.md", AGENTS)]);
    std::fs::remove_dir_all(disk(&sandbox, "a")).unwrap();

    eject(&sandbox, &[disk(&sandbox, "a/AGENTS.md")])
        .assert()
        .code(0);

    assert!(!exists(mirror(&sandbox, "a")));
    assert_eq!(
        read(backed_up(&sandbox, "repo/projects/work/a/AGENTS.md")),
        AGENTS
    );
    assert!(!exists(disk(&sandbox, "a")), "nothing is created on disk");
    assert!(!synced_text(&sandbox).contains("\"a/AGENTS.md\""));
    sync_is_quiet(&sandbox);
}

#[test]
fn eject_of_an_item_whose_mirror_is_gone_forgets_it() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    std::fs::remove_dir_all(mirror(&sandbox, "a")).unwrap();
    // Deleted in the mirror: sync reports it until it is ejected.
    sandbox.skiman().arg("sync").assert().code(3);

    eject(&sandbox, &[disk(&sandbox, "a/AGENTS.md")])
        .assert()
        .code(0);

    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), AGENTS, "untouched");
    assert!(
        !exists(mirror(&sandbox, "a")),
        "the mirror is not brought back"
    );
    assert!(!synced_text(&sandbox).contains("\"a/AGENTS.md\""));
    sync_is_quiet(&sandbox);
}

#[test]
fn an_exact_path_that_is_an_alias_link_names_the_link_not_its_target() {
    let sandbox = adopted(&[("p/AGENTS.md", "rules\n")]);
    std::os::unix::fs::symlink("AGENTS.md", disk(&sandbox, "p/CLAUDE.md")).unwrap();
    write(mirror(&sandbox, "p/AGENTS.md"), "mirror rules\n");
    write(mirror(&sandbox, "p/CLAUDE.md"), "mirror claude\n");

    let assert = sandbox
        .skiman()
        .args(["project", "restore", "--yes"])
        .arg(disk(&sandbox, "p/CLAUDE.md"))
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("p/CLAUDE.md is not a regular file"),
        "{stderr}"
    );
    assert_eq!(read(disk(&sandbox, "p/AGENTS.md")), "rules\n");
    assert!(
        std::fs::symlink_metadata(disk(&sandbox, "p/CLAUDE.md"))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn a_restore_path_that_names_nothing_is_refused() {
    let sandbox = adopted(&[("p/AGENTS.md", "rules\n")]);
    let assert = sandbox
        .skiman()
        .args(["project", "restore", "--yes"])
        .arg(disk(&sandbox, "no-such-folder"))
        .assert()
        .code(1);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("is neither a project item nor a folder holding one"),
        "{stderr}"
    );
}
