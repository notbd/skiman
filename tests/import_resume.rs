//! Resuming `skiman import <folder>` after it stopped part-way (spec Import,
//! resume paragraph; story 72).
//!
//! A precise step is hit with the test-only hook: `SKIMAN_TEST_HOOK` names
//! `import:<point>` and `SKIMAN_TEST_HOOK_RUN` is the shell command debug
//! builds run there; `exit 1` makes the import stop at that step (a panic,
//! exit 101). Points: `rename:<repo path>` before each repo piece is renamed
//! into place (after an existing original went to the backup), and
//! `back-up:`, `moved:` and `link:` + `<backup path>` around each location's
//! takeover (before the move, after the move before the record is saved,
//! before the link).

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use support::{Sandbox, Tool, tree};

fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: The {name} skill\n---\n\n# {name}\n")
}

/// A setup with instruction files that are regular files, a skill copied
/// into one tool directory and linked from another, a copy the import
/// leaves out, and an import folder holding own skill `notes` and
/// third-party skill `pdf` (pinned, with a local edit).
fn setup(sandbox: &Sandbox) -> PathBuf {
    write(
        sandbox.instruction_file(Tool::ClaudeCode),
        "old claude rules\n",
    );
    write(sandbox.instruction_file(Tool::Codex), "old codex rules\n");
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write(claude.join("notes/SKILL.md"), skill_md("notes"));
    write(claude.join("old-copy/SKILL.md"), skill_md("old-copy"));
    std::os::unix::fs::symlink(
        claude.join("notes"),
        sandbox.tool_dir(Tool::Codex).join("notes"),
    )
    .unwrap();

    let up = sandbox.upstream("acme/skills");
    up.write("skills/pdf/SKILL.md", skill_md("pdf"))
        .write("skills/pdf/reference.md", "# Reference\n");
    // A fixed date gives every sandbox the same commit, so final states compare.
    up.git(["add", "--all"]);
    let committed = sandbox
        .std_command("git")
        .current_dir(up.path())
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.test"])
        .args(["commit", "--quiet", "-m", "v1"])
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .status()
        .unwrap();
    assert!(committed.success());
    let commit = up.head();

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(3);
    write(
        sandbox.skill_repo().join("instructions/shared.md"),
        "old shared body\n",
    );

    let folder = sandbox.root().join("import");
    write(folder.join("skills/notes/SKILL.md"), skill_md("notes"));
    write(
        folder.join("skills/pdf/SKILL.md"),
        format!("{}\nMy local note.\n", skill_md("pdf")),
    );
    write(folder.join("skills/pdf/reference.md"), "# Reference\n");
    write(folder.join("instructions/shared.md"), "# Shared\n");
    write(
        folder.join("skiman.toml"),
        format!(
            "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n\n[skills.pdf.source]\nurl = \"https://github.com/acme/skills.git\"\npath = \"skills/pdf\"\nref = \"{commit}\"\n"
        ),
    );
    folder
}

fn import(sandbox: &Sandbox, folder: &Path) -> std::process::Output {
    sandbox
        .skiman()
        .arg("import")
        .arg(folder)
        .arg("--yes")
        .output()
        .unwrap()
}

/// Stops the import at `point` (exit 101, the panic of the test hook).
fn import_stopped_at(sandbox: &Sandbox, folder: &Path, point: &str) {
    let output = sandbox
        .skiman()
        .arg("import")
        .arg(folder)
        .arg("--yes")
        .env("SKIMAN_TEST_HOOK", format!("import:{point}"))
        .env("SKIMAN_TEST_HOOK_RUN", "exit 1")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(101),
        "{point}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The skill repo and the home folder (tool directories, instruction files,
/// machine state with the backup), with the sandbox root and the backup
/// folder name replaced, so two sandboxes compare equal. The git cache is
/// left out.
fn final_state(sandbox: &Sandbox) -> BTreeMap<String, String> {
    let backups = sandbox.state_home().join("skiman/backups");
    let stamps: Vec<String> = std::fs::read_dir(&backups)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(stamps.len(), 1, "one backup folder: {stamps:?}");
    let root = sandbox.root().display().to_string();
    let normalise = |s: &str| s.replace(&root, "<root>").replace(&stamps[0], "<ts>");
    let mut out = BTreeMap::new();
    for (prefix, dir) in [("repo", sandbox.skill_repo()), ("home", sandbox.home())] {
        for (path, value) in tree(&dir) {
            let key = format!("{prefix}/{}", path.display());
            if key.starts_with("home/.local/state/skiman/cache") {
                continue;
            }
            out.insert(normalise(&key), normalise(&value));
        }
    }
    out
}

/// The final state of an import that was never interrupted.
fn uninterrupted() -> BTreeMap<String, String> {
    let sandbox = Sandbox::new();
    let folder = setup(&sandbox);
    let output = import(&sandbox, &folder);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    final_state(&sandbox)
}

/// Stops the import at `point`, removes every upstream (so a fetch would
/// fail), resumes it and returns the final state.
fn stopped_and_resumed(point: &str) -> BTreeMap<String, String> {
    let sandbox = Sandbox::new();
    let folder = setup(&sandbox);
    let folder_before = tree(&folder);
    import_stopped_at(&sandbox, &folder, point);
    std::fs::remove_dir_all(sandbox.upstreams_dir()).unwrap();

    let output = import(&sandbox, &folder);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{point}: {stdout}\n{stderr}");
    assert!(
        stdout.contains("resuming the import of"),
        "{point}: {stdout}"
    );
    assert!(
        stdout.ends_with(&format!(
            "the import folder {} can be deleted\n",
            folder.display()
        )),
        "{point}: {stdout}"
    );
    assert!(
        !sandbox.state_home().join("skiman/import/area").exists(),
        "{point}: the temporary area is still there"
    );
    assert_eq!(tree(&folder), folder_before, "{point}");
    final_state(&sandbox)
}

#[track_caller]
fn assert_same(point: &str, expected: &BTreeMap<String, String>) {
    let actual = stopped_and_resumed(point);
    let differ: Vec<_> = expected
        .iter()
        .filter(|(k, v)| actual.get(*k) != Some(v))
        .map(|(k, _)| k)
        .chain(actual.keys().filter(|k| !expected.contains_key(*k)))
        .collect();
    let shown: Vec<_> = differ
        .iter()
        .map(|k| {
            format!(
                "{k}:\n  expected {:?}\n  actual   {:?}",
                expected.get(*k),
                actual.get(*k)
            )
        })
        .collect();
    assert!(
        differ.is_empty(),
        "{point}: differs from an uninterrupted import:\n{}",
        shown.join("\n")
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_stop_before_each_skill_then_resume_ends_like_an_uninterrupted_import() {
    let expected = uninterrupted();
    for point in ["rename:skills/notes", "rename:skills/pdf"] {
        assert_same(point, &expected);
    }
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_stop_before_upstream_lock_shared_body_or_manifest_then_resume_ends_like_an_uninterrupted_import()
 {
    let expected = uninterrupted();
    for point in [
        "rename:upstream/pdf",
        "rename:skiman.lock",
        "rename:instructions/shared.md",
        "rename:skiman.toml",
    ] {
        assert_same(point, &expected);
    }
}

/// The takeover order: tool-directory entries, then instruction locations.
const LOCATIONS: [&str; 5] = [
    "claude-code/notes",
    "claude-code/old-copy",
    "codex/notes",
    "claude-code/CLAUDE.md",
    "codex/AGENTS.md",
];

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_stop_around_each_location_takeover_then_resume_ends_like_an_uninterrupted_import() {
    let expected = uninterrupted();
    for location in LOCATIONS {
        for step in ["back-up", "moved", "link"] {
            assert_same(&format!("{step}:{location}"), &expected);
        }
    }
}

/// The skill repo, the home folder (tool directories, instruction files,
/// machine state without the git cache) and the import folder, to prove a
/// command changed nothing.
fn world(sandbox: &Sandbox) -> Vec<BTreeMap<PathBuf, String>> {
    let mut home = tree(sandbox.home());
    home.retain(|path, _| !path.starts_with(".local/state/skiman/cache"));
    vec![
        tree(sandbox.skill_repo()),
        home,
        tree(sandbox.root().join("import")),
    ]
}

/// Runs the import and asserts it refuses (exit 1) with every text in
/// `stderr_has`, changing nothing.
#[track_caller]
fn assert_resume_refused(sandbox: &Sandbox, folder: &Path, stderr_has: &[&str]) -> String {
    let before = world(sandbox);
    let output = import(sandbox, folder);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    for text in stderr_has {
        assert!(stderr.contains(text), "{text:?} not in {stderr}");
    }
    assert_eq!(world(sandbox), before, "{stderr}");
    stderr
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_changed_import_folder_refuses_the_resume_naming_the_difference() {
    let sandbox = Sandbox::new();
    let folder = setup(&sandbox);
    import_stopped_at(&sandbox, &folder, "rename:skills/notes");
    let original = read(folder.join("skills/notes/SKILL.md"));

    write(folder.join("skills/notes/SKILL.md"), "edited later\n");
    write(folder.join("skills/extra/SKILL.md"), skill_md("extra"));
    let stderr = assert_resume_refused(
        &sandbox,
        &folder,
        &[
            "the import folder changed since the import started",
            "  skills/extra is new\n",
            "  skills/notes changed\n",
        ],
    );
    assert!(!stderr.contains("skills/pdf"), "{stderr}");

    std::fs::remove_dir_all(folder.join("skills/extra")).unwrap();
    write(folder.join("skills/notes/SKILL.md"), &original);
    write(folder.join("instructions/shared.md"), "edited later\n");
    assert_resume_refused(&sandbox, &folder, &["  instructions/shared.md changed"]);

    write(folder.join("instructions/shared.md"), "# Shared\n");
    let manifest = read(folder.join("skiman.toml"));
    write(folder.join("skiman.toml"), format!("{manifest}# edited\n"));
    assert_resume_refused(&sandbox, &folder, &["  skiman.toml, or an entry"]);

    // Put back as it was, the import resumes.
    write(folder.join("skiman.toml"), &manifest);
    let output = import(&sandbox, &folder);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn repo_content_changed_by_hand_after_the_stop_refuses_the_resume_naming_it() {
    let sandbox = Sandbox::new();
    let folder = setup(&sandbox);
    let repo = sandbox.skill_repo();
    // upstream/pdf, skiman.lock, shared.md and skiman.toml are written;
    // skills/notes is next.
    import_stopped_at(&sandbox, &folder, "rename:skills/notes");

    let manifest = read(repo.join("skiman.toml"));
    write(repo.join("skiman.toml"), format!("{manifest}# hand edit\n"));
    write(repo.join("upstream/pdf/extra.md"), "hand edit\n");
    write(repo.join("skills/mine/SKILL.md"), skill_md("mine"));
    write(
        repo.join("skills/notes/SKILL.md"),
        format!("{}\nMade by hand.\n", skill_md("notes")),
    );
    assert_resume_refused(
        &sandbox,
        &folder,
        &[
            "the skill repo",
            "changed since the import stopped",
            "  upstream/pdf differs from what the import wrote\n",
            "  skiman.toml differs from both its content before the import and what the import writes\n",
            "  skills/notes differs from what the import wrote\n",
            "  skills/mine was not written by this import\n",
        ],
    );

    // Undone by hand, the import resumes and ends complete.
    write(repo.join("skiman.toml"), &manifest);
    std::fs::remove_file(repo.join("upstream/pdf/extra.md")).unwrap();
    std::fs::remove_dir_all(repo.join("skills/mine")).unwrap();
    std::fs::remove_dir_all(repo.join("skills/notes")).unwrap();
    let output = import(&sandbox, &folder);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(read(repo.join("skills/notes/SKILL.md")), skill_md("notes"));
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn originals_backed_up_before_the_stop_are_still_in_the_backup_after_the_resume() {
    let sandbox = Sandbox::new();
    let folder = setup(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    import_stopped_at(&sandbox, &folder, "link:codex/notes");
    let record_file = sandbox.state_home().join("skiman/import/record.toml");
    let record: toml_edit::DocumentMut = read(&record_file).parse().unwrap();
    let backup = PathBuf::from(record["backup"].as_str().unwrap());
    let backed_up = tree(&backup);
    assert_eq!(
        read(backup.join("claude-code/notes/SKILL.md")),
        skill_md("notes")
    );
    assert_eq!(
        read(backup.join("repo/instructions/shared.md")),
        "old shared body\n"
    );

    let output = import(&sandbox, &folder);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains(&format!("; the originals are in {}\n", backup.display())),
        "{stdout}"
    );
    let after = tree(&backup);
    for (path, value) in &backed_up {
        assert_eq!(after.get(path), Some(value), "{}", path.display());
    }
    assert_eq!(
        read(backup.join("claude-code/CLAUDE.md")),
        "old claude rules\n"
    );
    support::assert_symlink(
        claude.join("notes"),
        sandbox.skill_repo().join("skills/notes"),
        sandbox.skill_repo().join("skills/notes"),
    );
    let record: toml_edit::DocumentMut = read(&record_file).parse().unwrap();
    assert_eq!(record["status"].as_str(), Some("finished"));
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn while_an_import_is_unfinished_importing_another_folder_is_refused() {
    let sandbox = Sandbox::new();
    let folder = setup(&sandbox);
    import_stopped_at(&sandbox, &folder, "rename:skills/notes");
    let other = sandbox.root().join("other");
    write(other.join("skills/todo/SKILL.md"), skill_md("todo"));
    write(other.join("instructions/shared.md"), "Other.\n");
    write(other.join("skiman.toml"), "");

    let before = world(&sandbox);
    let output = import(&sandbox, &other);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "an import of {} into {} stopped part-way",
            folder.display(),
            sandbox.skill_repo().display()
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "finish it first with `skiman import {}`",
            folder.display()
        )),
        "{stderr}"
    );
    assert_eq!(world(&sandbox), before);
}
