//! `skiman update` failure paths (spec Merge semantics; clarifications 1, 2
//! and 7 on #1): re-hash mismatches, failures before the lock entry with
//! the printed recovery, a failed old-folder backup after the lock entry,
//! and an unwritable backups directory.
//!
//! A precise step is hit with the test-only hook: `SKIMAN_TEST_HOOK` names
//! `<skill>:<point>` and `SKIMAN_TEST_HOOK_RUN` is the shell command debug
//! builds run there.

mod support;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use support::{Sandbox, Tool, Upstream, tree};

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

fn write(path: impl AsRef<Path>, text: &str) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Twenty numbered lines, so edits far apart merge cleanly.
fn lines(edits: &[(usize, &str)]) -> String {
    (1..=20)
        .map(|n| {
            let line = edits
                .iter()
                .find(|(at, _)| *at == n)
                .map_or_else(|| format!("line {n}"), |(_, text)| (*text).to_owned());
            format!("{line}\n")
        })
        .collect()
}

/// Upstream `acme/<name>` (one skill at its root), installed as `<name>`.
fn installed(sandbox: &Sandbox, name: &str) -> Upstream {
    let up = sandbox.upstream(&format!("acme/{name}"));
    up.write(
        "SKILL.md",
        format!("---\nname: {name}\ndescription: The {name} skill\n---\n"),
    )
    .write("reference.md", lines(&[]))
    .write("forms.md", lines(&[]));
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", &format!("acme/{name}"), "--yes"])
        .assert()
        .code(0);
    up
}

/// `pdf` installed, with an uncommitted local edit, and a new upstream
/// version that merges cleanly with it.
fn pdf_with_an_update(sandbox: &Sandbox) -> Upstream {
    init(sandbox);
    let up = installed(sandbox, "pdf");
    write(
        sandbox.skill_repo().join("skills/pdf/forms.md"),
        &lines(&[(1, "uncommitted edit")]),
    );
    up.write("reference.md", lines(&[(20, "upstream fix")]));
    up.commit("v2");
    up
}

fn backups(sandbox: &Sandbox) -> PathBuf {
    sandbox.state_home().join("skiman/backups")
}

fn backup_dirs(sandbox: &Sandbox) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(backups(sandbox)) {
        Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    };
    dirs.sort();
    dirs
}

/// `skiman update <names>` with the test hook set to run `command` at
/// `<skill>:<point>`.
fn update_with_hook(
    sandbox: &Sandbox,
    names: &[&str],
    at: &str,
    command: &str,
) -> snapbox::cmd::OutputAssert {
    sandbox
        .skiman()
        .arg("update")
        .args(names)
        .env("SKIMAN_TEST_HOOK", at)
        .env("SKIMAN_TEST_HOOK_RUN", command)
        .assert()
}

const NEWER: &str = "edit made during the update\n";

/// Each re-hash mismatch (clarification 7): exit 4, nothing swapped, the
/// newer edit kept, no recovery printed.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_edit_before_the_first_re_hash_stops_the_skill_with_nothing_swapped_or_saved() {
    let sandbox = Sandbox::new();
    pdf_with_an_update(&sandbox);
    let repo = sandbox.skill_repo();
    let forms = repo.join("skills/pdf/forms.md");
    let mut expected = tree(&repo);

    update_with_hook(
        &sandbox,
        &["pdf"],
        "pdf:before-rehash-1",
        &format!("printf '{}' > '{}'", NEWER.trim_end(), forms.display()),
    )
    .code(4)
    .stdout_eq("")
    .stderr_eq(
        "error: skill pdf: skills/pdf changed during the update, so the update of pdf stopped and nothing was swapped; your newer edits are kept\n",
    );

    expected.insert(
        "skills/pdf/forms.md".into(),
        format!("file 644 {}", NEWER.trim_end()),
    );
    assert_eq!(tree(&repo), expected);
    assert!(backup_dirs(&sandbox).is_empty());
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_edit_between_the_two_re_hashes_stops_the_skill_with_only_the_records_saved() {
    let sandbox = Sandbox::new();
    pdf_with_an_update(&sandbox);
    let repo = sandbox.skill_repo();
    let forms = repo.join("skills/pdf/forms.md");
    let mut expected = tree(&repo);

    update_with_hook(
        &sandbox,
        &["pdf"],
        "pdf:before-rehash-2",
        &format!("printf '{}' > '{}'", NEWER.trim_end(), forms.display()),
    )
    .code(4)
    .stdout_eq("")
    .stderr_eq(
        "error: skill pdf: skills/pdf changed during the update, so the update of pdf stopped and nothing was swapped (its records were saved to [..]); your newer edits are kept\n",
    );

    expected.insert(
        "skills/pdf/forms.md".into(),
        format!("file 644 {}", NEWER.trim_end()),
    );
    assert_eq!(tree(&repo), expected);
    let dirs = backup_dirs(&sandbox);
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    assert_eq!(
        tree(&dirs[0]).into_keys().collect::<Vec<_>>(),
        [
            "repo",
            "repo/skiman.lock.pdf.fragment",
            "repo/skiman.toml.pdf.fragment"
        ]
        .map(PathBuf::from),
    );
}

fn chmod(path: impl AsRef<Path>, mode: u32) {
    std::fs::set_permissions(path.as_ref(), std::fs::Permissions::from_mode(mode)).unwrap();
}

/// The skill repo without `.git` and `.skiman` (what recovery restores).
fn content(sandbox: &Sandbox) -> BTreeMap<PathBuf, String> {
    let mut all = tree(sandbox.skill_repo());
    all.retain(|path, _| !path.starts_with(".git") && !path.starts_with(".skiman"));
    all
}

/// Replaces the `[skills.<name>]` table and its subtables in `text` with
/// `fragment`, the way a user pastes a fragment back by hand.
fn paste(text: &str, name: &str, fragment: &str) -> String {
    let ours = |line: &str| {
        line == format!("[skills.{name}]") || line.starts_with(&format!("[skills.{name}."))
    };
    let mut out = String::new();
    let mut inside = false;
    let mut pasted = false;
    for line in text.split_inclusive('\n') {
        let header = line.trim_end();
        if header.starts_with('[') {
            inside = ours(header);
            if inside && !pasted {
                out.push_str(fragment);
                pasted = true;
            } else if !inside && pasted && !out.ends_with("\n\n") {
                out.push('\n');
            }
        }
        if !inside {
            out.push_str(line);
        }
    }
    assert!(pasted, "no [skills.{name}] in:\n{text}");
    out
}

/// Follows the recovery printed in `stderr` step by step. `leftovers` is
/// where a later command moved the tmp leftovers (its `moved tmp leftovers
/// to <path>` line), if one did.
fn follow_recovery(sandbox: &Sandbox, stderr: &str, leftovers: Option<&Path>) {
    let repo = sandbox.skill_repo();
    let tmp = repo.join(".skiman/tmp");
    let steps: Vec<&str> = stderr
        .lines()
        .skip_while(|l| !l.starts_with("  to undo the update"))
        .skip(1)
        .take_while(|l| l.starts_with("  ") && l[2..].starts_with(char::is_numeric))
        .map(|l| l.trim_start().split_once(". ").unwrap().1)
        .collect();
    assert!(!steps.is_empty(), "no recovery in:\n{stderr}");
    for step in steps {
        if let Some(rest) = step.strip_prefix("move ") {
            let (from, to) = rest.split_once(" to ").unwrap();
            let (from, optional) = match from.strip_suffix(" (if it exists)") {
                Some(from) => (from, true),
                None => (from, false),
            };
            let mut from = PathBuf::from(from);
            if let (Some(moved), Ok(rel)) = (leftovers, from.strip_prefix(&tmp)) {
                from = moved.join(rel);
            }
            if optional && std::fs::symlink_metadata(&from).is_err() {
                continue;
            }
            std::fs::create_dir_all(Path::new(to).parent().unwrap()).unwrap();
            std::fs::rename(&from, to)
                .unwrap_or_else(|e| panic!("move {} to {to}: {e}", from.display()));
        } else if let Some(rest) = step.strip_prefix("in ") {
            let (file, rest) = rest.split_once(", replace the ").unwrap();
            let (_, fragment) = rest.rsplit_once(" with ").unwrap();
            let name = file_skill(rest);
            let path = repo.join(file);
            write(&path, &paste(&read(&path), &name, &read(fragment)));
        } else if step == "run `skiman sync`" {
            sandbox.skiman().arg("sync").assert().code(0);
        } else {
            panic!("unknown recovery step: {step}");
        }
    }
}

/// The skill name in `[skills.<name>] ...`.
fn file_skill(text: &str) -> String {
    let start = text.find("[skills.").unwrap() + "[skills.".len();
    let end = start + text[start..].find(']').unwrap();
    text[start..end].to_owned()
}

/// A failure before the lock entry: exit 4, the completed steps, the backup
/// folder and the recovery; following it restores the pre-update state with
/// the uncommitted edit, and the other skill is never touched.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn following_the_recovery_after_a_failed_lock_write_restores_the_skill_as_it_was() {
    let sandbox = Sandbox::new();
    pdf_with_an_update(&sandbox);
    installed(&sandbox, "docx");
    let repo = sandbox.skill_repo();
    let before = content(&sandbox);
    let tools_before = tree(sandbox.tool_dir(Tool::ClaudeCode));

    let assert = update_with_hook(
        &sandbox,
        &["pdf"],
        "pdf:before-lock",
        &format!("chmod 555 '{}'", repo.display()),
    )
    .code(4)
    .stdout_eq("");
    chmod(&repo, 0o755);
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    let backup = backup_dirs(&sandbox).pop().unwrap();
    let r = repo.display().to_string();
    let b = backup.display().to_string();
    let staging = std::fs::read_dir(repo.join(".skiman/tmp"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let s = staging.display().to_string();
    let ts = staging.file_name().unwrap().to_str().unwrap().to_owned();
    assert_eq!(
        stderr,
        format!(
            "error: skill pdf: cannot write {r}/skiman.lock: Permission denied (os error 13)\n\
             \x20 done: renamed {r}/skills/pdf to {s}/old/skills/pdf\n\
             \x20 done: renamed {r}/upstream/pdf to {s}/old/upstream/pdf\n\
             \x20 done: renamed {s}/new/upstream/pdf to {r}/upstream/pdf\n\
             \x20 done: renamed {s}/new/skills/pdf to {r}/skills/pdf\n\
             \x20 backup folder: {b}\n\
             \x20 to undo the update of pdf, in this order:\n\
             \x20 1. move {r}/upstream/pdf (if it exists) to {b}/aside/upstream/pdf\n\
             \x20 2. move {r}/skills/pdf (if it exists) to {b}/aside/skills/pdf\n\
             \x20 3. move {s}/old/skills/pdf to {r}/skills/pdf\n\
             \x20 4. move {s}/old/upstream/pdf to {r}/upstream/pdf\n\
             \x20 5. in skiman.toml, replace the [skills.pdf] table and its subtables with {b}/repo/skiman.toml.pdf.fragment\n\
             \x20 6. in skiman.lock, replace the [skills.pdf] entry with {b}/repo/skiman.lock.pdf.fragment\n\
             \x20 7. run `skiman sync`\n\
             \x20 if a later command moved the tmp leftovers to a backup (it printed `moved tmp leftovers to <path>`), take the folders of steps 3 and 4 from <path>/{ts}/old/ instead\n"
        )
    );

    follow_recovery(&sandbox, &stderr, None);

    assert_eq!(content(&sandbox), before);
    assert_eq!(tree(sandbox.tool_dir(Tool::ClaudeCode)), tools_before);
    sandbox.skiman().arg("check").assert().code(0);
}

fn stdout_of(assert: &snapbox::cmd::OutputAssert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

fn stderr_of(assert: &snapbox::cmd::OutputAssert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

/// The command's `tmp/<timestamp>/` folder (the only entry of `.skiman/tmp/`).
fn staging_dir(sandbox: &Sandbox) -> PathBuf {
    let entries: Vec<PathBuf> = std::fs::read_dir(sandbox.skill_repo().join(".skiman/tmp"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    entries[0].clone()
}

/// The path in a `moved tmp leftovers to <path>` line.
fn moved_leftovers(stdout: &str) -> PathBuf {
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("moved tmp leftovers to "))
        .unwrap_or_else(|| panic!("no tmp leftovers moved:\n{stdout}"));
    PathBuf::from(line)
}

/// Clarifications 2 and 7: the lock entry is written, so the update is
/// complete; the old folders stay in `.skiman/tmp/` as tmp leftovers, named
/// on stderr, listed by `doctor`, and moved to a backup by the next write
/// command, with every edit kept.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_failed_old_folder_backup_after_the_lock_entry_leaves_tmp_leftovers_and_keeps_the_exit_code() {
    let sandbox = Sandbox::new();
    let up = pdf_with_an_update(&sandbox);
    let repo = sandbox.skill_repo();
    let skill_before = tree(repo.join("skills/pdf"));
    let upstream_before = tree(repo.join("upstream/pdf"));

    let assert = update_with_hook(
        &sandbox,
        &["pdf"],
        "pdf:before-backup-move",
        "chmod 555 \"$XDG_STATE_HOME\"/skiman/backups/*/repo",
    )
    .code(0)
    .stdout_eq(format!(
        "skill pdf: updated to {}; changed files:\n  modified reference.md\n\
         saved the records of pdf to [..]\n",
        up.head()
    ));
    let backup = backup_dirs(&sandbox).pop().unwrap();
    chmod(backup.join("repo"), 0o755);
    let s = staging_dir(&sandbox).display().to_string();
    assert_eq!(
        stderr_of(&assert),
        format!(
            "warning: skill pdf: cannot move {s}/old/skills/pdf to the backup (Permission denied (os error 13)); it stays in .skiman/tmp/ as a tmp leftover\n\
             warning: skill pdf: cannot move {s}/old/upstream/pdf to the backup (Permission denied (os error 13)); it stays in .skiman/tmp/ as a tmp leftover\n"
        )
    );
    assert_eq!(
        read(repo.join("skills/pdf/reference.md")),
        lines(&[(20, "upstream fix")])
    );
    assert!(read(repo.join("skiman.lock")).contains(&up.head()));

    let doctor = sandbox.skiman().arg("doctor").assert().code(0);
    let report = stdout_of(&doctor);
    assert!(
        report.contains(&format!(
            "tmp leftover {s}; the next write command moves it to a backup\n"
        )),
        "{report}"
    );

    one_skill_upstream(&sandbox, "docx");
    let add = sandbox
        .skiman()
        .args(["add", "acme/docx", "--yes"])
        .assert()
        .code(0);
    let moved = moved_leftovers(&stdout_of(&add));
    assert!(moved.starts_with(backups(&sandbox)), "{}", moved.display());
    let ts = Path::new(&s).file_name().unwrap();
    assert_eq!(tree(moved.join(ts).join("old/skills/pdf")), skill_before);
    assert_eq!(
        tree(moved.join(ts).join("old/upstream/pdf")),
        upstream_before
    );
    assert_eq!(
        std::fs::read_dir(repo.join(".skiman/tmp")).unwrap().count(),
        0,
        "no tmp leftovers left"
    );
}

/// The non-blank lines of the `[skills.<name>]` table and its subtables in `text`.
fn section(text: &str, name: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with('[') {
            inside =
                line == format!("[skills.{name}]") || line.starts_with(&format!("[skills.{name}."));
        }
        if inside && !line.is_empty() {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The spec scenario: in a multi-skill update the first skill succeeds and
/// the second fails part-way (a rename blocked by a folder recreated
/// meanwhile); a later command moves the tmp leftovers to a backup; the
/// second skill is recovered per the printed instructions without losing its
/// uncommitted edits or the first skill's update.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_skill_that_failed_part_way_is_recovered_after_a_later_command_moved_the_tmp_leftovers() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let docx = installed(&sandbox, "docx");
    let pdf = installed(&sandbox, "pdf");
    let repo = sandbox.skill_repo();
    for name in ["docx", "pdf"] {
        write(
            repo.join(format!("skills/{name}/forms.md")),
            &lines(&[(1, "uncommitted edit")]),
        );
    }
    for up in [&docx, &pdf] {
        up.write("reference.md", lines(&[(20, "upstream fix")]));
        up.commit("v2");
    }
    let pdf_skill = tree(repo.join("skills/pdf"));
    let pdf_upstream = tree(repo.join("upstream/pdf"));
    let manifest = read(repo.join("skiman.toml"));
    let pdf_lock = section(&read(repo.join("skiman.lock")), "pdf");
    let recreated = repo.join("skills/pdf");

    let assert = update_with_hook(
        &sandbox,
        &[],
        "pdf:before-rename-4",
        &format!(
            "mkdir '{0}' && printf 'recreated\\n' > '{0}/notes.md'",
            recreated.display()
        ),
    )
    .code(4)
    .stdout_eq(format!(
        "skill docx: updated to {}; changed files:\n  modified reference.md\n\
         saved the old folders and records of docx to [..]\n...\n",
        docx.head()
    ));
    let stderr = stderr_of(&assert);
    let r = repo.display();
    let s = staging_dir(&sandbox).display().to_string();
    let pdf_backup = backup_dirs(&sandbox).pop().unwrap();
    assert!(
        stderr.starts_with(&format!(
            "error: skill pdf: {r}/skills/pdf appeared while skiman was writing; nothing was put in its place\n\
             \x20 done: renamed {r}/skills/pdf to {s}/old/skills/pdf\n\
             \x20 done: renamed {r}/upstream/pdf to {s}/old/upstream/pdf\n\
             \x20 done: renamed {s}/new/upstream/pdf to {r}/upstream/pdf\n\
             \x20 backup folder: {}\n\
             \x20 to undo the update of pdf, in this order:\n",
            pdf_backup.display()
        )),
        "{stderr}"
    );

    one_skill_upstream(&sandbox, "xlsx");
    let add = sandbox
        .skiman()
        .args(["add", "acme/xlsx", "--yes"])
        .assert()
        .code(0);
    let moved = moved_leftovers(&stdout_of(&add));
    follow_recovery(&sandbox, &stderr, Some(&moved));

    assert_eq!(tree(repo.join("skills/pdf")), pdf_skill);
    assert_eq!(tree(repo.join("upstream/pdf")), pdf_upstream);
    assert_eq!(section(&read(repo.join("skiman.lock")), "pdf"), pdf_lock);
    assert_eq!(
        section(&read(repo.join("skiman.toml")), "pdf"),
        section(&manifest, "pdf")
    );
    assert_eq!(
        read(pdf_backup.join("aside/skills/pdf/notes.md")),
        "recreated\n"
    );
    assert_eq!(
        read(repo.join("skills/docx/reference.md")),
        lines(&[(20, "upstream fix")])
    );
    assert_eq!(
        read(repo.join("skills/docx/forms.md")),
        lines(&[(1, "uncommitted edit")])
    );
    assert!(section(&read(repo.join("skiman.lock")), "docx").contains(&docx.head()));
    assert!(
        sandbox.tool_dir(Tool::ClaudeCode).join("pdf").is_symlink(),
        "sync distributes the recovered pdf again"
    );
    sandbox.skiman().arg("check").assert().code(0);
}

/// Upstream `acme/<name>` with only a `SKILL.md`, not installed.
fn one_skill_upstream(sandbox: &Sandbox, name: &str) {
    let up = sandbox.upstream(&format!("acme/{name}"));
    up.write(
        "SKILL.md",
        format!("---\nname: {name}\ndescription: The {name} skill\n---\n"),
    );
    up.commit("v1");
}

/// A failed backup stops the skill before anything is swapped.
#[test]
fn an_unwritable_backups_directory_stops_the_skill_before_anything_is_swapped() {
    let sandbox = Sandbox::new();
    pdf_with_an_update(&sandbox);
    let before = tree(sandbox.skill_repo());
    std::fs::create_dir_all(backups(&sandbox)).unwrap();
    chmod(backups(&sandbox), 0o555);

    let assert = sandbox.skiman().args(["update", "pdf"]).assert();
    chmod(backups(&sandbox), 0o755);
    assert.code(4).stdout_eq("").stderr_eq(
        "error: skill pdf: cannot save the records of pdf to a backup: Permission denied (os error 13); nothing of pdf changed\n",
    );

    assert_eq!(tree(sandbox.skill_repo()), before);
    assert!(backup_dirs(&sandbox).is_empty());
}

/// Same snapshot, new commit: only the records advance; a failed lock write
/// prints the fragment recovery.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_failed_lock_write_when_only_the_records_advance_prints_the_fragment_recovery() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = installed(&sandbox, "pdf");
    up.commit("an empty commit");
    let repo = sandbox.skill_repo();
    let before = content(&sandbox);

    let assert = update_with_hook(
        &sandbox,
        &["pdf"],
        "pdf:before-lock",
        &format!("chmod 555 '{}'", repo.display()),
    )
    .code(4)
    .stdout_eq("");
    chmod(&repo, 0o755);
    let stderr = stderr_of(&assert);
    let r = repo.display();
    let b = backup_dirs(&sandbox).pop().unwrap();
    let b = b.display();
    assert_eq!(
        stderr,
        format!(
            "error: skill pdf: cannot write {r}/skiman.lock: Permission denied (os error 13)\n\
             \x20 backup folder: {b}\n\
             \x20 to undo the update of pdf, in this order:\n\
             \x20 1. in skiman.toml, replace the [skills.pdf] table and its subtables with {b}/repo/skiman.toml.pdf.fragment\n\
             \x20 2. in skiman.lock, replace the [skills.pdf] entry with {b}/repo/skiman.lock.pdf.fragment\n\
             \x20 3. run `skiman sync`\n"
        )
    );

    follow_recovery(&sandbox, &stderr, None);
    assert_eq!(content(&sandbox), before);
}

/// With `--ref`, `skiman.toml` is written before the lock entry; a failed
/// lock write lists it as done and the recovery pastes its fragment back.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn following_the_recovery_after_a_ref_change_restores_skiman_toml_too() {
    let sandbox = Sandbox::new();
    pdf_with_an_update(&sandbox).branch("stable");
    let repo = sandbox.skill_repo();
    let manifest = read(repo.join("skiman.toml"));
    let before = content(&sandbox);

    let assert = sandbox
        .skiman()
        .args(["update", "pdf", "--ref", "stable"])
        .env("SKIMAN_TEST_HOOK", "pdf:before-lock")
        .env(
            "SKIMAN_TEST_HOOK_RUN",
            format!("chmod 555 '{}'", repo.display()),
        )
        .assert();
    chmod(&repo, 0o755);
    let assert = assert.code(4).stdout_eq("");
    let stderr = stderr_of(&assert);
    assert!(
        stderr.contains(
            "\n  done: wrote the new `source.ref` and `ref_kind` of pdf to skiman.toml\n"
        ),
        "{stderr}"
    );
    assert_ne!(read(repo.join("skiman.toml")), manifest);

    follow_recovery(&sandbox, &stderr, None);
    assert_eq!(content(&sandbox), before);
}
