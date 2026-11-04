//! `skiman remove <name>`: move a skill's folders to a backup, drop its
//! records and its tool-directory links.

mod support;

use std::path::{Path, PathBuf};

use snapbox::str;
use support::{Sandbox, Tool, tree};

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

fn exists(path: impl AsRef<Path>) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// The skill repo (without `.git` and `.skiman`) and every tool directory.
fn world(sandbox: &Sandbox) -> Vec<std::collections::BTreeMap<PathBuf, String>> {
    let repo = sandbox.skill_repo();
    let mut world = vec![
        tree(repo.join("skills")),
        tree(repo.join("upstream")),
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

/// A third-party skill `pdf` from `acme/pdf`, installed by `add`.
fn add_pdf(sandbox: &Sandbox) {
    let up = sandbox.upstream("acme/pdf");
    up.write(
        "SKILL.md",
        "---\nname: pdf\ndescription: Fill PDF forms\n---\n\n# pdf\n",
    )
    .write("reference.md", "# Reference\n");
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
}

#[test]
fn remove_of_an_own_skill_moves_its_folder_to_a_backup_and_removes_its_links() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);
    let repo = sandbox.skill_repo();
    // Uncommitted edits are kept in the backup.
    std::fs::write(repo.join("skills/notes/draft.md"), "not committed\n").unwrap();

    sandbox
        .skiman()
        .args(["remove", "notes", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
moved [..]/skill-repo/skills/notes to [..]/state/skiman/backups/[..]/repo/skills/notes
removed skill notes; its folders are in [..]/state/skiman/backups/[..]
removed [..]/home/.claude/skills/notes
removed [..]/home/.agents/skills/notes
removed [..]/home/.gemini/config/skills/notes

"#]])
        .stderr_eq("");

    let backup = only_backup(&sandbox);
    assert_eq!(
        read(backup.join("repo/skills/notes/draft.md")),
        "not committed\n"
    );
    assert!(backup.join("repo/skills/notes/SKILL.md").is_file());
    assert!(!exists(repo.join("skills/notes")));
    for tool in Tool::ALL {
        assert!(!exists(sandbox.tool_dir(tool).join("notes")));
    }
    check_is_clean(&sandbox);
}

#[test]
fn remove_of_a_third_party_skill_moves_both_folders_and_removes_its_records() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let repo = sandbox.skill_repo();
    std::fs::write(repo.join("skills/pdf/reference.md"), "# Edited\n").unwrap();
    let manifest_before = read(repo.join("skiman.toml"));

    sandbox
        .skiman()
        .args(["remove", "pdf", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
moved [..]/skill-repo/skills/pdf to [..]/state/skiman/backups/[..]/repo/skills/pdf
moved [..]/skill-repo/upstream/pdf to [..]/state/skiman/backups/[..]/repo/upstream/pdf
changed [..]/skill-repo/skiman.toml
changed [..]/skill-repo/skiman.lock
removed skill pdf; its folders are in [..]/state/skiman/backups/[..]
removed [..]/home/.claude/skills/pdf
removed [..]/home/.agents/skills/pdf
removed [..]/home/.gemini/config/skills/pdf

"#]])
        .stderr_eq("");

    let backup = only_backup(&sandbox);
    assert_eq!(
        read(backup.join("repo/skills/pdf/reference.md")),
        "# Edited\n"
    );
    assert_eq!(
        read(backup.join("repo/upstream/pdf/reference.md")),
        "# Reference\n"
    );
    assert!(!exists(repo.join("skills/pdf")));
    assert!(!exists(repo.join("upstream/pdf")));
    // Only the skill's table is gone; the rest keeps its bytes.
    let manifest = read(repo.join("skiman.toml"));
    assert!(!manifest.contains("pdf"), "{manifest}");
    assert!(
        manifest_before.starts_with(manifest.trim_end()),
        "{manifest}"
    );
    assert!(!read(repo.join("skiman.lock")).contains("pdf"));
    for tool in Tool::ALL {
        assert!(!exists(sandbox.tool_dir(tool).join("pdf")));
    }
    check_is_clean(&sandbox);
}

#[test]
fn remove_of_an_unknown_name_exits_1_and_changes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["remove", "nothing", "--yes"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: there is no skill nothing: no skills/nothing, no upstream/nothing, no [skills.nothing] table in skiman.toml and no entry in skiman.lock

"#]]);
    // A name that is not one path component never reaches the file system.
    sandbox
        .skiman()
        .args(["remove", "..", "--yes"])
        .assert()
        .code(1);

    assert_eq!(world(&sandbox), before);
    assert!(!exists(sandbox.state_home().join("skiman/backups")));
}

#[test]
fn without_a_terminal_remove_needs_yes() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["remove", "pdf"])
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: missing flag without a terminal: --yes

"#]]);
    assert_eq!(world(&sandbox), before);
}

#[test]
fn with_a_terminal_remove_asks_and_a_no_changes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let before = world(&sandbox);

    let mut pty = sandbox.skiman_pty(["remove", "pdf"]);
    let shown = pty.expect("Remove skill pdf? [y/N] ");
    assert!(shown.contains("move to a backup: "), "{shown}");
    pty.send_line("n");
    let (code, rest) = pty.wait();
    assert_eq!(code, 1, "{rest}");
    assert!(rest.contains("not removed; nothing was changed"), "{rest}");
    assert_eq!(world(&sandbox), before);

    let mut pty = sandbox.skiman_pty(["remove", "pdf"]);
    pty.expect("Remove skill pdf? [y/N] ");
    pty.send_line("y");
    let (code, rest) = pty.wait();
    assert_eq!(code, 0, "{rest}");
    assert!(!exists(sandbox.skill_repo().join("skills/pdf")));
}

#[test]
fn with_an_unwritable_backups_folder_remove_stops_and_the_records_stay() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let backups = sandbox.state_home().join("skiman/backups");
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o555)).unwrap();
    let before = world(&sandbox);

    let assert = sandbox
        .skiman()
        .args(["remove", "pdf", "--yes"])
        .assert()
        .code(4)
        .stdout_eq("");
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        stderr.contains("cannot create a backup folder in"),
        "{stderr}"
    );
    assert!(stderr.contains("nothing was removed"), "{stderr}");
    assert_eq!(world(&sandbox), before);
}

#[test]
fn remove_after_a_hand_delete_cleans_up_the_leftover_records() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let repo = sandbox.skill_repo();
    std::fs::remove_dir_all(repo.join("skills/pdf")).unwrap();
    let check = sandbox.skiman().arg("check").assert().code(3);
    let report = String::from_utf8_lossy(&check.get_output().stdout).into_owned();
    assert!(report.contains("run `skiman remove pdf --yes`"), "{report}");

    sandbox
        .skiman()
        .args(["remove", "pdf", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
moved [..]/skill-repo/upstream/pdf to [..]/state/skiman/backups/[..]/repo/upstream/pdf
changed [..]/skill-repo/skiman.toml
changed [..]/skill-repo/skiman.lock
removed skill pdf; its folders are in [..]/state/skiman/backups/[..]
removed [..]/home/.claude/skills/pdf
removed [..]/home/.agents/skills/pdf
removed [..]/home/.gemini/config/skills/pdf

"#]])
        .stderr_eq("");

    assert!(!exists(repo.join("upstream/pdf")));
    check_is_clean(&sandbox);
}

#[test]
fn remove_of_records_without_any_folder_writes_no_backup() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let repo = sandbox.skill_repo();
    std::fs::remove_dir_all(repo.join("skills/pdf")).unwrap();
    std::fs::remove_dir_all(repo.join("upstream/pdf")).unwrap();

    sandbox
        .skiman()
        .args(["remove", "pdf", "--yes"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
changed [..]/skill-repo/skiman.toml
changed [..]/skill-repo/skiman.lock
removed the records of skill pdf
removed [..]/home/.claude/skills/pdf
removed [..]/home/.agents/skills/pdf
removed [..]/home/.gemini/config/skills/pdf

"#]])
        .stderr_eq("");
    check_is_clean(&sandbox);
}
