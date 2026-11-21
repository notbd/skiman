//! `skiman project restore` and `skiman project eject` (project-scope.md
//! "Commands", "Writing: one protocol for every replace"). Restore under a
//! folder writes only what this machine never had; named exactly it also
//! replaces, with the project's version in `backups/*/projects/<id>/...`. Eject
//! moves the mirror to `backups/*/repo/projects/...` and never touches the
//! project.

use std::path::{Path, PathBuf};

use snapbox::cmd::Command;
use snapbox::str;

use crate::support::{Sandbox, backups, exists, init, killed_at, read, skill_md, tree, write};

const AGENTS: &str = "# a\n\nUse pnpm.\n";

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

/// `projects/work/<rel>` in the canon.
fn mirror(sandbox: &Sandbox, rel: &str) -> PathBuf {
    sandbox.canon().join("projects/work").join(rel)
}

fn synced_file(sandbox: &Sandbox) -> PathBuf {
    sandbox.state_home().join("skiman/projects-synced.toml")
}

/// A canon with `~/work` registered; `files` are written under the
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
    let Ok(entries) = std::fs::read_dir(backups(sandbox)) else {
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

    sandbox
        .skiman_confined()
        .args(["project", "restore", "--yes"])
        .arg(root(&sandbox))
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
moved tmp leftovers to [..]/home/.local/state/skiman/backups/[..]/projects/work/a
...
"#]]);

    assert!(!exists(&leftover));
    // Moved as it was, next to where the backups of the anchor's items go.
    let kept: Vec<(String, String)> = tree(backups(&sandbox))
        .into_iter()
        .filter(|(path, entry)| {
            path.to_string_lossy().contains("/old/") && entry.starts_with("file")
        })
        .map(|(path, entry)| (path.to_string_lossy().into_owned(), entry))
        .collect();
    let [(path, entry)] = &kept[..] else {
        panic!("{kept:?}");
    };
    assert!(
        path.ends_with("/projects/work/a/.skiman-swap-20200101T000000Z/old/AGENTS.md"),
        "{path}"
    );
    assert_eq!(entry, "file 644 only copy\n");
    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), "from the mirror\n");
    assert_eq!(swap_folders(&disk(&sandbox, "a")), Vec::<PathBuf>::new());
}

/// A restore whose replaced version cannot go to the backup keeps it in the
/// anchor's staging folder, out of the project's git, and says so; the next
/// restore there moves it to a backup.
#[test]
fn a_previous_version_the_backup_refuses_stays_out_of_the_project_git_and_moves_on() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = adopted(&[("g/AGENTS.md", AGENTS)]);
    let repo = disk(&sandbox, "g");
    sandbox.git(&repo, ["init", "-q", "-b", "main"]);
    write(mirror(&sandbox, "g/AGENTS.md"), "from the mirror\n");
    let backups = backups(&sandbox);
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o555)).unwrap();
    let failed = restore(&sandbox, &[repo.join("AGENTS.md")])
        .output()
        .unwrap();
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(failed.status.code(), Some(4));
    assert_eq!(read(repo.join("AGENTS.md")), "from the mirror\n");
    let [leftover] = &swap_folders(&repo)[..] else {
        panic!("{:?}", swap_folders(&repo));
    };
    let kept = leftover.join("old/AGENTS.md");
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(
        stderr.contains("is restored, but its previous version cannot be moved to the backup (")
            && stderr.contains(&format!("); it is kept at {}\n", kept.display())),
        "{stderr}"
    );
    assert_eq!(read(&kept), AGENTS);
    assert_eq!(
        sandbox.git(&repo, ["status", "--porcelain", "--untracked-files=all"]),
        "?? AGENTS.md"
    );

    write(mirror(&sandbox, "g/AGENTS.md"), "from the mirror again\n");
    restore(&sandbox, &[repo.join("AGENTS.md")])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
write the mirror's version over [..]/home/work/g/AGENTS.md
moved tmp leftovers to [..]/home/.local/state/skiman/backups/[..]/projects/work/g
...
"#]]);
    assert_eq!(swap_folders(&repo), Vec::<PathBuf>::new());
    let name = leftover.file_name().unwrap().to_string_lossy();
    let moved = backed_up(&sandbox, &format!("projects/work/g/{name}/old/AGENTS.md"));
    assert_eq!(read(moved), AGENTS);
}

/// A tmp leftover in the anchor that cannot be moved to the backup stays
/// where it is, says so once, and the restore writes both items all the same.
#[test]
fn a_tmp_leftover_that_cannot_be_moved_does_not_stop_restore() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS), ("a/CLAUDE.md", AGENTS)]);
    write(mirror(&sandbox, "a/AGENTS.md"), "from the mirror\n");
    write(mirror(&sandbox, "a/CLAUDE.md"), "also from the mirror\n");
    let leftover = disk(&sandbox, "a/.skiman-swap-20200101T000000Z");
    write(leftover.join("old/AGENTS.md"), "only copy\n");
    // Neither the folder nor what is in `old/` can be moved out.
    let mode = |m| {
        for dir in [leftover.join("old"), leftover.clone()] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(m)).unwrap();
        }
    };
    mode(0o555);
    let items = [disk(&sandbox, "a/AGENTS.md"), disk(&sandbox, "a/CLAUDE.md")];
    let out = restore(&sandbox, &items).output().unwrap();
    mode(0o755);

    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), "from the mirror\n");
    assert_eq!(
        read(disk(&sandbox, "a/CLAUDE.md")),
        "also from the mirror\n"
    );
    let said = String::from_utf8_lossy(&out.stdout);
    let named = format!(
        "cannot move the tmp leftover {} to a backup: ",
        leftover.display()
    );
    assert_eq!(said.matches(&named).count(), 1, "{said}");
    assert_eq!(read(leftover.join("old/AGENTS.md")), "only copy\n");
}

/// An anchor where no staging folder can be made fails its item only: the
/// restore writes the item under the next anchor, saves the synced record
/// and says where the replaced versions are.
#[test]
fn an_anchor_that_cannot_hold_a_staging_folder_fails_its_item_only() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS), ("b/AGENTS.md", AGENTS)]);
    write(mirror(&sandbox, "a/AGENTS.md"), "a from the mirror\n");
    write(mirror(&sandbox, "b/AGENTS.md"), "b from the mirror\n");
    let before = synced_text(&sandbox);
    let mode =
        |m| std::fs::set_permissions(disk(&sandbox, "a"), std::fs::Permissions::from_mode(m));
    mode(0o555).unwrap();
    let items = [disk(&sandbox, "a/AGENTS.md"), disk(&sandbox, "b/AGENTS.md")];
    let out = restore(&sandbox, &items).output().unwrap();
    mode(0o755).unwrap();

    let (said, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(out.status.code(), Some(4), "{said}{stderr}");
    assert!(stderr.contains("cannot create a folder in"), "{stderr}");
    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), AGENTS);
    assert_eq!(read(disk(&sandbox, "b/AGENTS.md")), "b from the mirror\n");
    assert!(said.contains("replaced versions are in "), "{said}");
    assert_ne!(
        synced_text(&sandbox),
        before,
        "the synced record of b/AGENTS.md is saved"
    );
}

/// Only a real folder named like a staging folder is a tmp leftover: a
/// link of the user's with such a name stays where it is, and so does what
/// it points to.
#[test]
fn a_link_named_like_a_staging_folder_is_not_a_tmp_leftover() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    write(mirror(&sandbox, "a/AGENTS.md"), "from the mirror\n");
    let theirs = sandbox.root().join("theirs");
    write(theirs.join("notes.md"), "the user's\n");
    let link = disk(&sandbox, "a/.skiman-swap-x");
    std::os::unix::fs::symlink(&theirs, &link).unwrap();

    let out = restore(&sandbox, &[disk(&sandbox, "a/AGENTS.md")])
        .output()
        .unwrap();

    let said = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(!said.contains("moved tmp leftovers"), "{said}");
    assert_eq!(std::fs::read_link(&link).unwrap(), theirs);
    assert_eq!(read(theirs.join("notes.md")), "the user's\n");
}

/// A restore killed before its write leaves a staging folder that holds
/// only its `.gitignore`: no content, so the run again deletes it instead of
/// moving it to a backup.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_staging_folder_with_only_its_gitignore_is_deleted_not_backed_up() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    write(mirror(&sandbox, "a/AGENTS.md"), "from the mirror\n");
    let item = disk(&sandbox, "a/AGENTS.md");
    let args = ["project", "restore", "--yes", &item.to_string_lossy()].map(String::from);
    killed_at(&sandbox, &args, "project:write:a/AGENTS.md");
    let [left] = &swap_folders(&disk(&sandbox, "a"))[..] else {
        panic!("{:?}", swap_folders(&disk(&sandbox, "a")));
    };
    assert_eq!(
        tree(left).into_keys().collect::<Vec<_>>(),
        [PathBuf::from(".gitignore")]
    );

    let again = restore(&sandbox, std::slice::from_ref(&item))
        .output()
        .unwrap();

    let said = String::from_utf8_lossy(&again.stdout);
    assert_eq!(again.status.code(), Some(0), "{said}");
    assert!(!said.contains("moved tmp leftovers"), "{said}");
    assert_eq!(read(&item), "from the mirror\n");
    assert_eq!(swap_folders(&disk(&sandbox, "a")), Vec::<PathBuf>::new());
}

/// An item under an anchor that takes most of `PATH_MAX` is restored: the
/// staging folder beside the item holds it by its name, so the anchor is in
/// the staged path once, as in the item's own.
#[test]
fn an_item_under_a_deep_anchor_is_restored() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    // Two thirds of the room left: the anchor twice would not fit.
    let path_max = if cfg!(target_os = "macos") {
        1024
    } else {
        4096
    };
    let room = path_max - root(&sandbox).as_os_str().len();
    let anchor: PathBuf = (0..room * 2 / 3 / 101).map(|_| "d".repeat(100)).collect();
    let rel = anchor.join("AGENTS.md");
    let rel = rel.to_str().unwrap();
    write(disk(&sandbox, rel), AGENTS);
    let skiman = |args: &[&str]| {
        sandbox
            .skiman()
            .args(args)
            .arg(root(&sandbox))
            .assert()
            .code(0)
    };
    skiman(&["project", "add"]);
    skiman(&["project", "adopt", "--yes"]);
    write(mirror(&sandbox, rel), "from the mirror\n");

    restore(&sandbox, &[disk(&sandbox, rel)]).assert().code(0);

    assert_eq!(read(disk(&sandbox, rel)), "from the mirror\n");
    assert_eq!(
        swap_folders(&disk(&sandbox, anchor.to_str().unwrap())),
        Vec::<PathBuf>::new()
    );
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
            tree(backups(&sandbox)),
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

    sandbox
        .skiman_confined()
        .args(["project", "eject", "--yes"])
        .arg(disk(&sandbox, "a/sub/AGENTS.md"))
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

    let manifest = read(sandbox.canon().join("skiman.toml"));
    assert!(!manifest.contains("projects"), "{manifest}");
    assert!(!exists(sandbox.canon().join("projects")));
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

    let manifest = read(sandbox.canon().join("skiman.toml"));
    assert!(!manifest.contains("projects"), "{manifest}");
    assert!(!exists(sandbox.canon().join("projects")));
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

#[test]
fn eject_forgets_an_item_whose_mirror_is_already_gone() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    std::fs::remove_file(mirror(&sandbox, "a/AGENTS.md")).unwrap();
    sandbox
        .skiman()
        .args(["project", "eject", "--yes"])
        .arg(disk(&sandbox, "a/AGENTS.md"))
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
forget a/AGENTS.md in project root work (its mirror is already gone)
...
"#]]);
    assert!(!read(synced_file(&sandbox)).contains("a/AGENTS.md"));
    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), AGENTS);
}

#[test]
fn restore_writes_the_mirror_version_it_checked_not_a_later_edit() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    write(mirror(&sandbox, "a/AGENTS.md"), "checked\n");
    let item = disk(&sandbox, "a/AGENTS.md").display().to_string();

    let mut pty = sandbox.skiman_pty(["project", "restore", item.as_str()]);
    pty.expect("Restore 1 item(s)? [y/N] ");
    // An edit to the mirror while the user reads the plan.
    write(mirror(&sandbox, "a/AGENTS.md"), "<<<<<<< HEAD\nlater\n");
    pty.send_line("y");
    let (code, rest) = pty.wait();

    assert_eq!(code, 3, "{rest}");
    assert_eq!(read(disk(&sandbox, "a/AGENTS.md")), "checked\n");
}

#[test]
fn eject_never_moves_or_cleans_up_through_a_linked_mirror_folder() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    let outside = sandbox.root().join("outside");
    std::fs::rename(sandbox.canon().join("projects"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, sandbox.canon().join("projects")).unwrap();

    let assert = sandbox
        .skiman()
        .args(["project", "eject", "--yes"])
        .arg(root(&sandbox))
        .assert()
        .code(3);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("its mirror cannot be used"), "{stderr}");
    assert_eq!(read(outside.join("work/a/AGENTS.md")), AGENTS);
    assert!(
        read(sandbox.canon().join("skiman.toml")).contains("[projects.work]"),
        "the root stays registered"
    );
}

#[test]
fn eject_of_two_roots_finishes_the_first_when_the_second_has_a_linked_mirror() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    let other = sandbox.home().join("other");
    std::fs::create_dir_all(&other).unwrap();
    sandbox
        .skiman()
        .args(["project", "add", "--id", "other"])
        .arg(&other)
        .assert()
        .code(0);
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, sandbox.canon().join("projects/other")).unwrap();

    let assert = sandbox
        .skiman()
        .args(["project", "eject", "--yes"])
        .arg(root(&sandbox))
        .arg(&other)
        .assert()
        .code(3);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("[projects.other] is not removed"),
        "{stderr}"
    );
    let manifest = read(sandbox.canon().join("skiman.toml"));
    assert!(!manifest.contains("[projects.work]"), "{manifest}");
    assert!(manifest.contains("[projects.other]"), "{manifest}");
    assert!(!exists(synced_file(&sandbox)) || !read(synced_file(&sandbox)).contains("a/AGENTS.md"));
    assert!(elsewhere.exists(), "nothing cleaned up through the link");
}

#[test]
fn eject_keeps_a_root_whose_mirror_hides_items_behind_a_nested_link() {
    let sandbox = adopted(&[("a/AGENTS.md", AGENTS)]);
    let outside = sandbox.root().join("outside-a");
    std::fs::rename(mirror(&sandbox, "a"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, mirror(&sandbox, "a")).unwrap();

    let assert = sandbox
        .skiman()
        .args(["project", "eject", "--yes"])
        .arg(root(&sandbox))
        .assert()
        .code(3);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("is not forgotten: its mirror cannot be used"),
        "{stderr}"
    );
    assert!(read(sandbox.canon().join("skiman.toml")).contains("[projects.work]"));
    assert!(
        read(synced_file(&sandbox)).contains("a/AGENTS.md"),
        "the synced hash stays"
    );
    assert_eq!(read(outside.join("AGENTS.md")), AGENTS);
}
