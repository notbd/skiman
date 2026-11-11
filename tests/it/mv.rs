//! `skiman mv <old> <new>`: rename a skill everywhere it lives.
//!
//! Secret-like fixtures are built at runtime from pieces, so no literal token
//! is committed.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
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

fn append(path: impl AsRef<Path>, text: &str) {
    let mut old = read(&path);
    old.push_str(text);
    std::fs::write(path, old).unwrap();
}

fn exists(path: impl AsRef<Path>) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// The skill repo (without `.git`) and every tool directory.
fn world(sandbox: &Sandbox) -> Vec<std::collections::BTreeMap<PathBuf, String>> {
    let repo = sandbox.skill_repo();
    let mut world = vec![
        tree(repo.join("skills")),
        tree(repo.join("upstream")),
        tree(repo.join(".skiman/tmp")),
        [repo.join("skiman.toml"), repo.join("skiman.lock")]
            .iter()
            .map(|f| (f.clone(), std::fs::read_to_string(f).unwrap_or_default()))
            .collect(),
    ];
    world.extend(Tool::ALL.iter().map(|t| tree(sandbox.tool_dir(*t))));
    world
}

fn check_is_clean(sandbox: &Sandbox) {
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

fn assert_linked_only_as(sandbox: &Sandbox, new: &str, old: &str) {
    let target = sandbox.skill_repo().join("skills").join(new);
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join(new), &target, &target);
        assert!(!exists(sandbox.tool_dir(tool).join(old)), "{old} link left");
    }
}

fn aws_key() -> String {
    ["AK", "IA", "Q2W3E4R5T6Y7U8I9"].concat()
}

fn fingerprint(matched: &str) -> String {
    let hex: String = Sha256::digest(matched.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
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
fn mv_of_an_own_skill_renames_the_folder_name_table_and_secret_exemption() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);
    let repo = sandbox.skill_repo();
    std::fs::write(
        repo.join("skills/notes/keys.md"),
        format!("key {}\n", aws_key()),
    )
    .unwrap();
    append(
        repo.join("skiman.toml"),
        &format!(
            "\n[skills.notes]\nenabled = true\n\n[[secrets.allow]]\npath = \"skills/notes/keys.md\"\nrule = \"aws-access-key\"\nfingerprint = \"{}\"\n",
            fingerprint(&aws_key())
        ),
    );
    check_is_clean(&sandbox);

    sandbox
        .skiman()
        .args(["mv", "notes", "memo"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
renamed [..]/skill-repo/skills/notes to [..]/skill-repo/skills/memo
changed [..]/skill-repo/skills/memo/SKILL.md
changed [..]/skill-repo/skiman.toml
removed [..]/home/.claude/skills/notes
linked [..]/home/.claude/skills/memo
removed [..]/home/.agents/skills/notes
linked [..]/home/.agents/skills/memo
removed [..]/home/.gemini/config/skills/notes
linked [..]/home/.gemini/config/skills/memo

"#]])
        .stderr_eq("");

    assert!(!exists(repo.join("skills/notes")));
    assert_eq!(
        read(repo.join("skills/memo/SKILL.md")),
        "---\nname: memo\ndescription: \"Take notes\"\n---\n\n# notes\n",
        "only the name line changes"
    );
    let manifest = read(repo.join("skiman.toml"));
    assert!(
        manifest.contains("\n[skills.memo]\nenabled = true\n"),
        "{manifest}"
    );
    assert!(
        manifest.contains("path = \"skills/memo/keys.md\""),
        "{manifest}"
    );
    assert!(!manifest.contains("notes"), "{manifest}");
    assert_linked_only_as(&sandbox, "memo", "notes");
    check_is_clean(&sandbox);
}

#[test]
fn mv_of_a_third_party_skill_renames_its_records_and_snapshot_and_records_the_upstream_name() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let repo = sandbox.skill_repo();
    std::fs::write(repo.join("skills/pdf/reference.md"), "# Edited\n").unwrap();
    let lock_before = read(repo.join("skiman.lock"));

    sandbox
        .skiman()
        .args(["mv", "pdf", "forms"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
renamed [..]/skill-repo/skills/pdf to [..]/skill-repo/skills/forms
changed [..]/skill-repo/skills/forms/SKILL.md
renamed [..]/skill-repo/upstream/pdf to [..]/skill-repo/upstream/forms
changed [..]/skill-repo/skiman.toml
changed [..]/skill-repo/skiman.lock
removed [..]/home/.claude/skills/pdf
linked [..]/home/.claude/skills/forms
removed [..]/home/.agents/skills/pdf
linked [..]/home/.agents/skills/forms
removed [..]/home/.gemini/config/skills/pdf
linked [..]/home/.gemini/config/skills/forms

"#]])
        .stderr_eq("");

    assert_eq!(read(repo.join("skills/forms/reference.md")), "# Edited\n");
    assert_eq!(
        read(repo.join("upstream/forms/SKILL.md")).lines().nth(1),
        Some("name: pdf")
    );
    let manifest = read(repo.join("skiman.toml"));
    assert!(
        manifest.contains(
            "[skills.forms.source]\nurl = \"https://github.com/acme/pdf.git\"\npath = \"\"\nupstream_name = \"pdf\"\n"
        ),
        "{manifest}"
    );
    assert_eq!(
        read(repo.join("skiman.lock")),
        lock_before.replace("[skills.pdf]", "[skills.forms]"),
        "the entry is renamed, its values stay"
    );
    assert_linked_only_as(&sandbox, "forms", "pdf");
    check_is_clean(&sandbox);

    // Back to the upstream name: `upstream_name` is no longer needed.
    sandbox
        .skiman()
        .args(["mv", "forms", "pdf"])
        .assert()
        .code(0);
    let manifest = read(repo.join("skiman.toml"));
    assert!(!manifest.contains("upstream_name"), "{manifest}");
    assert_eq!(read(repo.join("skiman.lock")), lock_before);
    assert_linked_only_as(&sandbox, "pdf", "forms");
    check_is_clean(&sandbox);
}

#[test]
fn mv_to_an_invalid_reserved_or_taken_name_exits_1_and_changes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);
    let repo = sandbox.skill_repo();
    // `orphan` is taken by a lock entry alone.
    append(
        repo.join("skiman.lock"),
        &format!(
            "\n[skills.orphan]\nsnapshot = \"sha256:{}\"\n",
            "0".repeat(64)
        ),
    );
    let before = world(&sandbox);

    for (new, message) in [
        ("Notes", "is not a valid skill name"),
        ("managing-skills", "is a reserved skill name"),
        (".hidden", "is a reserved skill name"),
        (
            "notes",
            "the name `notes` is taken: skills/notes already exists",
        ),
        (
            "orphan",
            "the name `orphan` is taken: skiman.lock already has an entry for orphan",
        ),
        ("pdf", "the name `pdf` is taken: skills/pdf already exists"),
    ] {
        let assert = sandbox
            .skiman()
            .args(["mv", "pdf", new])
            .assert()
            .code(1)
            .stdout_eq("");
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains(message), "{new}: {stderr}");
        assert_eq!(world(&sandbox), before, "{new} changed something");
    }
}

#[test]
fn mv_of_a_name_without_a_skill_folder_exits_1_and_changes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["mv", "nothing", "something"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: skills/nothing is not a skill folder

"#]]);
    assert_eq!(world(&sandbox), before);
}
