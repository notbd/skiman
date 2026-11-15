//! `skiman mv <old> <new>`: rename a skill everywhere it lives.
//!
//! Secret-like fixtures are built at runtime from pieces, so no literal token
//! is committed.

use std::path::{Path, PathBuf};

use snapbox::str;

use crate::support::{
    Sandbox, Tool, assert_each_kill_finishes, assert_symlink, aws_key, check_is_clean, exists,
    fingerprint, init, killed_at, read, tree,
};

fn append(path: impl AsRef<Path>, text: &str) {
    let mut old = read(&path);
    old.push_str(text);
    std::fs::write(path, old).unwrap();
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

fn assert_linked_only_as(sandbox: &Sandbox, new: &str, old: &str) {
    let target = sandbox.skill_repo().join("skills").join(new);
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join(new), &target, &target);
        assert!(!exists(sandbox.tool_dir(tool).join(old)), "{old} link left");
    }
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
changed [..]/skill-repo/skills/notes/SKILL.md
changed [..]/skill-repo/skiman.toml
renamed [..]/skill-repo/skills/notes to [..]/skill-repo/skills/memo
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
fn mv_of_a_third_party_skill_renames_its_records_and_snapshot() {
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
changed [..]/skill-repo/skills/pdf/SKILL.md
changed [..]/skill-repo/skiman.toml
changed [..]/skill-repo/skiman.lock
renamed [..]/skill-repo/upstream/pdf to [..]/skill-repo/upstream/forms
renamed [..]/skill-repo/skills/pdf to [..]/skill-repo/skills/forms
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
            "[skills.forms.source]\nurl = \"https://github.com/acme/pdf.git\"\npath = \"\"\n"
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

    sandbox
        .skiman()
        .args(["mv", "forms", "pdf"])
        .assert()
        .code(0);
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

/// Killed at any step, a mv finishes when run again: its first write makes
/// `skills/<old>/SKILL.md` name `<new>`, which shows a mv stopped part-way,
/// and its last renames the folder.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_mv_killed_at_any_step_finishes_when_run_again() {
    assert_each_kill_finishes(
        |sandbox| {
            init(sandbox);
            add_pdf(sandbox);
            ["mv", "pdf", "forms"].map(String::from).to_vec()
        },
        &[
            "mv:write:skills/pdf/SKILL.md",
            "mv:write:skiman.toml",
            "mv:write:skiman.lock",
            "mv:write:upstream/forms",
            "mv:write:skills/forms",
        ],
    );
    // Run again after its first write, it says it finishes that mv.
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let args = ["mv", "pdf", "forms"].map(String::from).to_vec();
    killed_at(&sandbox, &args, "mv:write:skiman.toml");
    let again = sandbox.skiman().args(&args).assert().code(0);
    let stdout = String::from_utf8_lossy(&again.get_output().stdout).into_owned();
    assert!(
        stdout.starts_with("skills/pdf/SKILL.md already names `forms`: finishing the rename\n"),
        "{stdout}"
    );
}

/// A link where `skills/<new>` goes, even one that points nowhere, is no mv
/// stopped part-way: mv refuses (exit 1) and changes nothing.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_dangling_link_at_the_new_name_is_no_mv_stopped_part_way() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let args = ["mv", "pdf", "forms"].map(String::from).to_vec();
    killed_at(&sandbox, &args, "mv:write:skiman.lock");
    std::os::unix::fs::symlink("/nonexistent/x", sandbox.skill_repo().join("skills/forms"))
        .unwrap();
    let before = world(&sandbox);

    sandbox.skiman().args(&args).assert().code(1);

    assert_eq!(world(&sandbox), before);
}

/// States no mv leaves, where only the old name's records or snapshot are
/// left, are not taken for a mv stopped part-way: mv refuses (exit 1) and
/// changes nothing: a third-party `pdf` whose folder was deleted by hand,
/// moved onto an own skill; a `pdf` a remove killed part-way left as
/// records, moved onto a third-party `forms`; a hand-deleted own `notes`
/// with a table, moved onto a third-party `pdf`.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn mv_of_what_only_records_are_left_of_is_refused_and_changes_nothing() {
    type Setup = fn(&Sandbox) -> [&'static str; 2];
    let cases: [Setup; 3] = [
        |sandbox| {
            add_pdf(sandbox);
            std::fs::remove_dir_all(sandbox.skill_repo().join("skills/pdf")).unwrap();
            let new = ["new", "notes", "-d", "Take notes"];
            sandbox.skiman().args(new).assert().code(0);
            ["pdf", "notes"]
        },
        |sandbox| {
            add_pdf(sandbox);
            let up = sandbox.upstream("acme/forms");
            up.write("SKILL.md", "---\nname: forms\ndescription: Forms\n---\n");
            up.commit("v1");
            let add = ["add", "acme/forms", "--yes"];
            sandbox.skiman().args(add).assert().code(0);
            let args = ["remove", "pdf", "--yes"].map(String::from);
            killed_at(sandbox, &args, "remove:write:skiman.toml");
            ["pdf", "forms"]
        },
        |sandbox| {
            add_pdf(sandbox);
            let new = ["new", "notes", "-d", "Take notes"];
            sandbox.skiman().args(new).assert().code(0);
            let repo = sandbox.skill_repo();
            append(
                repo.join("skiman.toml"),
                "\n[skills.notes]\nenabled = true\n",
            );
            std::fs::remove_dir_all(repo.join("skills/notes")).unwrap();
            ["notes", "pdf"]
        },
    ];
    for setup in cases {
        let sandbox = Sandbox::new();
        init(&sandbox);
        let [old, new] = setup(&sandbox);
        let repo = sandbox.skill_repo();
        let records = || {
            let lock = std::fs::read_to_string(repo.join("skiman.lock")).unwrap_or_default();
            (
                read(repo.join("skiman.toml")),
                lock,
                tree(repo.join("upstream")),
            )
        };
        let before = records();

        let mv = sandbox.skiman().args(["mv", old, new]).assert().code(1);
        let stderr = String::from_utf8_lossy(&mv.get_output().stderr).into_owned();
        assert!(
            stderr.starts_with(&format!("error: skills/{old} is not a skill folder")),
            "{stderr}"
        );
        assert!(
            stderr.contains("`skiman sync` finishes the rest"),
            "{stderr}"
        );
        assert_eq!(records(), before, "mv {old} {new}");
    }
}

/// A `skills/<old>/SKILL.md` that names `<new>` is where a mv stops
/// part-way only while no record has both names: with the records of a
/// `<new>` of its own (its folder deleted by hand), mv refuses the taken
/// name and changes nothing.
#[test]
fn a_skill_md_naming_another_skill_is_no_mv_stopped_part_way() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    let up = sandbox.upstream("acme/forms");
    up.write("SKILL.md", "---\nname: forms\ndescription: Forms\n---\n");
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/forms", "--yes"])
        .assert()
        .code(0);
    let repo = sandbox.skill_repo();
    std::fs::remove_dir_all(repo.join("skills/forms")).unwrap();
    let skill_md = repo.join("skills/pdf/SKILL.md");
    let text = read(&skill_md).replace("name: pdf", "name: forms");
    std::fs::write(&skill_md, text).unwrap();
    let before = world(&sandbox);

    sandbox
        .skiman()
        .args(["mv", "pdf", "forms"])
        .assert()
        .code(1)
        .stderr_eq(
            "error: the name `forms` is taken: skiman.toml already has a [skills.forms] table\n",
        );
    assert_eq!(world(&sandbox), before);
}

#[test]
fn the_summary_group_origin_and_installed_date_move_with_the_skill() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);
    let manifest = sandbox.skill_repo().join("skiman.toml");
    let info = "summary = \"Keep notes\"\norigin = \"written here\"\ninstalled = 2026-01-31\n";
    append(&manifest, &format!("\n[skills.notes]\n{info}"));

    sandbox
        .skiman()
        .args(["mv", "notes", "journal"])
        .assert()
        .code(0);

    let text = read(&manifest);
    assert!(
        text.contains(&format!("[skills.journal]\n{info}")),
        "{text}"
    );
    assert!(!text.contains("[skills.notes]"), "{text}");
}
