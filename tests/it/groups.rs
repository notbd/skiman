//! Group folders (ADR 0008): a skill at `skills/<group>/<name>` is in that
//! group, a skill at `skills/<name>` in none. The rules on where a skill
//! sits, the commands that put a skill in a group or move it to another,
//! and what follows a skill there: its links, its install day, its secret
//! exemptions.

use std::path::Path;

use snapbox::str;

use crate::support::{
    Sandbox, Tool, assert_symlink, aws_key, fingerprint, import, init, read, setup, skill_md, tree,
    write,
};

fn check(sandbox: &Sandbox) -> snapbox::cmd::OutputAssert {
    sandbox.skiman().arg("check").assert()
}

fn skiman(sandbox: &Sandbox, args: &[&str]) -> snapbox::cmd::OutputAssert {
    sandbox.skiman().args(args).assert()
}

/// Every tool's link `name` points at `skills/<rel>`.
fn assert_linked(sandbox: &Sandbox, name: &str, rel: &str) {
    let target = sandbox.skill_repo().join("skills").join(rel);
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join(name), &target, &target);
    }
}

fn commit_on(sandbox: &Sandbox, day: &str) {
    let repo = sandbox.skill_repo();
    sandbox.git(&repo, ["add", "--all"]);
    sandbox
        .command("git")
        .arg("-C")
        .arg(&repo)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.test"])
        .args(["commit", "--quiet", "-m", "skills"])
        .env("GIT_AUTHOR_DATE", format!("{day}T12:00:00"))
        .env("GIT_COMMITTER_DATE", format!("{day}T12:00:00"))
        .assert()
        .code(0);
}

#[test]
fn check_reports_a_name_in_two_groups_a_bad_group_name_and_a_folder_in_a_group_without_skill_md() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skills = sandbox.skill_repo().join("skills");
    for rel in [
        "a/notes",
        "b/notes",
        "Bad_Group/pdf",
        "other/writing",
        "writing/memo",
        "a/Inner_Bad/fill",
        "b/pdf/form",
    ] {
        let name = Path::new(rel).file_name().unwrap().to_str().unwrap();
        write(skills.join(rel).join("SKILL.md"), skill_md(name));
    }
    write(skills.join("writing/drafts/README.md"), "no SKILL.md\n");

    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/Bad_Group: skill-folder: `Bad_Group` is not a valid group name: use 1 to 64 lowercase letters, digits and single hyphens between them
skills/a/Inner_Bad: skill-folder: `Inner_Bad` is not a valid group name: use 1 to 64 lowercase letters, digits and single hyphens between them
skills/a/notes: skill-folder: the skill name `notes` is in skills/a/notes and skills/b/notes; a name may be in one place only, whatever its group
skills/b/notes: skill-folder: the skill name `notes` is in skills/a/notes and skills/b/notes; a name may be in one place only, whatever its group
skills/b/pdf: skill-folder: `pdf` names both this group folder and the skill skills/Bad_Group/pdf; rename one of them
skills/writing: skill-folder: `writing` names both this group folder and the skill skills/other/writing; rename one of them
skills/writing/drafts: skill-md: no SKILL.md at the top of the skill folder

"#]]);
    // Neither copy of `notes` is distributed.
    for tool in Tool::ALL {
        assert!(!sandbox.tool_dir(tool).join("notes").exists());
    }
}

/// The frontmatter `name` must equal the skill's own folder, not its group.
#[test]
fn a_grouped_skill_md_names_its_own_folder() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skill_md_path = sandbox.skill_repo().join("skills/docs/pdf/SKILL.md");
    write(&skill_md_path, skill_md("pdf"));
    check(&sandbox).code(0).stdout_eq("");

    write(&skill_md_path, skill_md("docs"));
    check(&sandbox).code(1).stdout_eq(str![[r#"
skills/docs/pdf/SKILL.md:2: name-folder: `name` is `docs` but the folder is `pdf`; they must be equal

"#]]);
}

#[test]
fn new_puts_a_skill_in_a_group_and_refuses_a_bad_or_taken_group() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    skiman(
        &sandbox,
        &["new", "notes", "-d", "Take notes", "--group", "writing"],
    )
    .code(0);
    let repo = sandbox.skill_repo();
    assert!(repo.join("skills/writing/notes/SKILL.md").is_file());
    assert_linked(&sandbox, "notes", "writing/notes");
    skiman(
        &sandbox,
        &[
            "new",
            "essay",
            "-d",
            "Write essays",
            "--group",
            "writing/long",
        ],
    )
    .code(0);
    assert!(repo.join("skills/writing/long/essay/SKILL.md").is_file());
    assert_linked(&sandbox, "essay", "writing/long/essay");

    const BAD: &str = "give one or more group folder names joined with `/`, each 1 to 64 lowercase letters, digits and single hyphens between them (^[a-z0-9]+(-[a-z0-9]+)*$)";
    for (args, error) in [
        (
            ["new", "memo", "-d", "d", "--group", "Bad"],
            format!("error: `Bad` is not a valid group: {BAD}\n"),
        ),
        (
            ["new", "memo", "-d", "d", "--group", "writing//x"],
            format!("error: `writing//x` is not a valid group: {BAD}\n"),
        ),
        (
            ["new", "memo", "-d", "d", "--group", "notes"],
            "error: `notes` cannot be a group: skills/writing/notes is a skill folder named `notes`\n".to_owned(),
        ),
        (
            ["new", "memo", "-d", "d", "--group", "other/notes"],
            "error: `other/notes` cannot be a group: skills/writing/notes is a skill folder named `notes`\n".to_owned(),
        ),
        (
            ["new", "notes", "-d", "d", "--group", "other"],
            "error: the name `notes` is taken: skills/writing/notes already exists\n".to_owned(),
        ),
        (
            ["new", "writing", "-d", "d", "--group", "other"],
            "error: the name `writing` is taken: skills/writing is a group folder\n".to_owned(),
        ),
        (
            ["new", "long", "-d", "d", "--group", "other"],
            "error: the name `long` is taken: skills/writing/long is a group folder\n".to_owned(),
        ),
        (
            ["new", "memo", "-d", "d", "--group", "other/memo"],
            "error: `other/memo` cannot be the group of a skill named `memo`: a group folder and a skill never share a name\n".to_owned(),
        ),
    ] {
        skiman(&sandbox, &args).code(1).stderr_eq(error);
    }
    assert!(!repo.join("skills/other").exists() && !repo.join("skills/memo").exists());
}

/// A move to another group renames only the folder: the records stay as
/// they are, the links follow, and the group folder it leaves empty goes.
#[test]
fn mv_moves_a_skill_between_groups_and_its_links_follow() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    skiman(
        &sandbox,
        &["new", "notes", "-d", "Take notes", "--group", "a"],
    )
    .code(0);
    std::fs::write(
        repo.join("skiman.toml"),
        read(repo.join("skiman.toml")) + "\n[skills.notes]\nsummary = \"Keep notes\"\n",
    )
    .unwrap();
    let manifest = read(repo.join("skiman.toml"));
    let content = tree(repo.join("skills/a/notes"));

    skiman(&sandbox, &["mv", "notes", "--group", "b"])
        .code(0)
        .stdout_eq(str![[r#"
renamed [..]/skill-repo/skills/a/notes to [..]/skill-repo/skills/b/notes
removed the empty group folder [..]/skill-repo/skills/a
...
"#]]);
    assert_eq!(tree(repo.join("skills/b/notes")), content);
    assert!(!repo.join("skills/a").exists());
    assert_eq!(read(repo.join("skiman.toml")), manifest);
    assert_linked(&sandbox, "notes", "b/notes");

    // Run again, it finds the move done.
    skiman(&sandbox, &["mv", "notes", "--group", "b"])
        .code(0)
        .stdout_eq("skills/b/notes is already in place\n");

    skiman(&sandbox, &["mv", "notes", "--no-group"]).code(0);
    assert_linked(&sandbox, "notes", "notes");

    // A rename and a move at once.
    skiman(&sandbox, &["mv", "notes", "memo", "--group", "c"]).code(0);
    assert!(read(repo.join("skills/c/memo/SKILL.md")).contains("name: memo"));
    assert!(read(repo.join("skiman.toml")).contains("[skills.memo]"));
    assert_linked(&sandbox, "memo", "c/memo");
    assert!(!sandbox.tool_dir(Tool::ClaudeCode).join("notes").exists());

    skiman(&sandbox, &["mv", "memo", "--group", "No"]).code(1);
    skiman(&sandbox, &["mv", "memo", "--group", "memo"]).code(1);
    skiman(&sandbox, &["mv", "memo", "note", "--group", "note"]).code(1);
    // A group is not a skill: remove refuses it and keeps what is in it.
    skiman(&sandbox, &["remove", "c", "--yes"])
        .code(1)
        .stderr_eq(
            "error: skills/c is a group folder, not a skill; remove its skills one by one\n",
        );
    assert!(repo.join("skills/c/memo/SKILL.md").is_file());
    skiman(&sandbox, &["mv", "memo"]).code(2);
    assert_linked(&sandbox, "memo", "c/memo");
}

/// Groups nest: a move into `a/b` makes both folders, and a move out takes
/// each group folder it leaves empty, innermost first, and no other.
#[test]
fn mv_moves_a_skill_into_and_out_of_nested_groups() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    for (name, group) in [("notes", "a/b"), ("memo", "a")] {
        skiman(&sandbox, &["new", name, "-d", "d", "--group", group]).code(0);
    }
    assert_linked(&sandbox, "notes", "a/b/notes");

    skiman(&sandbox, &["mv", "notes", "--group", "c/d"])
        .code(0)
        .stdout_eq(str![[r#"
renamed [..]/skill-repo/skills/a/b/notes to [..]/skill-repo/skills/c/d/notes
removed the empty group folder [..]/skill-repo/skills/a/b
...
"#]]);
    assert!(repo.join("skills/a/memo/SKILL.md").is_file());
    assert_linked(&sandbox, "notes", "c/d/notes");

    skiman(&sandbox, &["mv", "notes", "--no-group"])
        .code(0)
        .stdout_eq(str![[r#"
renamed [..]/skill-repo/skills/c/d/notes to [..]/skill-repo/skills/notes
removed the empty group folder [..]/skill-repo/skills/c/d
removed the empty group folder [..]/skill-repo/skills/c
...
"#]]);
    assert!(!repo.join("skills/c").exists());
    assert_linked(&sandbox, "notes", "notes");

    // A group folder's name is taken like a skill's, at any depth.
    skiman(&sandbox, &["mv", "notes", "--group", "a/b"]).code(0);
    skiman(&sandbox, &["mv", "memo", "b"])
        .code(1)
        .stderr_eq("error: the name `b` is taken: skills/a/b is a group folder\n");
    skiman(&sandbox, &["remove", "b", "--yes"])
        .code(1)
        .stderr_eq(
            "error: skills/a/b is a group folder, not a skill; remove its skills one by one\n",
        );
    skiman(&sandbox, &["mv", "memo", "--group", "x/notes"]).code(1);
    check(&sandbox).code(0);
}

/// `git log --follow` sees a move to another group as a rename: the install
/// day stays the day the skill was added.
#[test]
fn the_install_day_follows_a_move_to_another_group() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    skiman(
        &sandbox,
        &["new", "notes", "-d", "Take notes", "--group", "a"],
    )
    .code(0);
    commit_on(&sandbox, "2026-01-05");
    skiman(&sandbox, &["mv", "notes", "--group", "b"]).code(0);
    commit_on(&sandbox, "2026-02-01");

    let output = sandbox.skiman().arg("overview").output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("### b (1)")
            && stdout.contains("| notes | - | own | - | 2026-01-05 (git) | yes | - |"),
        "{stdout}"
    );
}

/// An exemption names the skill without its group, so it holds after the
/// skill moves to another group; one written with the group holds too.
#[test]
fn a_secret_exemption_names_the_skill_without_its_group() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    write(repo.join("skills/docs/pdf/SKILL.md"), skill_md("pdf"));
    let key = aws_key();
    write(
        repo.join("skills/docs/pdf/fixture.env"),
        format!("AWS_ACCESS_KEY_ID={key}\n"),
    );
    let fp = fingerprint(&key);
    let entry = format!(
        "\n[[secrets.allow]]\npath = \"skills/pdf/fixture.env\"\nrule = \"aws-access-key\"\nfingerprint = \"{fp}\"\n"
    );
    let output = sandbox.skiman().arg("check").output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("skills/docs/pdf/fixture.env:1: aws-access-key:"),
        "{stdout}"
    );
    assert!(stdout.contains(entry.trim_start()), "{stdout}");

    let manifest = repo.join("skiman.toml");
    std::fs::write(&manifest, read(&manifest) + &entry).unwrap();
    check(&sandbox).code(0).stdout_eq("");
    skiman(&sandbox, &["mv", "pdf", "--group", "forms/fill"]).code(0);
    check(&sandbox).code(0).stdout_eq("");

    let with_group = read(&manifest).replace("skills/pdf/", "skills/forms/fill/pdf/");
    std::fs::write(&manifest, with_group).unwrap();
    check(&sandbox).code(0).stdout_eq("");
}

/// A third-party skill added into a group: `list`, `diff` and `remove`
/// find it there, and `remove` takes the group folder it leaves empty.
#[test]
fn a_third_party_skill_in_a_group_is_listed_diffed_and_removed() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", skill_md("pdf"))
        .write("forms.md", "a\n");
    up.commit("v1");
    skiman(
        &sandbox,
        &["add", "acme/pdf", "--yes", "--group", "documents"],
    )
    .code(0);
    let repo = sandbox.skill_repo();
    assert_linked(&sandbox, "pdf", "documents/pdf");
    write(repo.join("skills/documents/pdf/forms.md"), "b\n");

    let list = sandbox.skiman().arg("list").output().unwrap();
    let list = String::from_utf8(list.stdout).unwrap();
    assert!(
        list.lines()
            .any(|l| l.starts_with("pdf ") && l.contains("third-party")),
        "{list}"
    );
    skiman(&sandbox, &["diff", "pdf"])
        .code(0)
        .stdout_eq(str![[r#"
--- upstream/pdf/forms.md
+++ skills/documents/pdf/forms.md
@@ -1 +1 @@
-a
+b

"#]]);

    // A move to another group keeps the snapshot, and says nothing about it.
    skiman(&sandbox, &["mv", "pdf", "--group", "forms"])
        .code(0)
        .stderr_eq("")
        .stdout_eq(str![[r#"
renamed [..]/skill-repo/skills/documents/pdf to [..]/skill-repo/skills/forms/pdf
removed the empty group folder [..]/skill-repo/skills/documents
retargeted [..]
...
"#]]);
    skiman(&sandbox, &["mv", "pdf", "--group", "documents"]).code(0);

    skiman(&sandbox, &["remove", "pdf", "--yes"]).code(0);
    assert!(!repo.join("skills/documents").exists());
    assert!(!repo.join("upstream/pdf").exists());
    assert!(!sandbox.tool_dir(Tool::ClaudeCode).join("pdf").exists());
}

#[test]
fn adopt_puts_the_copy_in_a_group() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let entry = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
    write(entry.join("SKILL.md"), skill_md("notes"));
    skiman(
        &sandbox,
        &[
            "adopt",
            "notes",
            "--from",
            "claude-code",
            "--yes",
            "--group",
            "writing",
        ],
    )
    .code(0);
    assert!(
        sandbox
            .skill_repo()
            .join("skills/writing/notes/SKILL.md")
            .is_file()
    );
    assert_linked(&sandbox, "notes", "writing/notes");
}

/// An import folder lays the skill repo out as it will be, group folders
/// included; a name in two groups refuses the import.
#[test]
fn import_keeps_the_group_folders_and_refuses_a_name_in_two_groups() {
    let sandbox = Sandbox::new();
    let folder = setup(&sandbox).folder;
    let skills = folder.join("skills");
    std::fs::create_dir(skills.join("writing")).unwrap();
    std::fs::rename(skills.join("notes"), skills.join("writing/notes")).unwrap();

    let output = import(&sandbox, &folder);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let repo = sandbox.skill_repo();
    assert!(repo.join("skills/writing/notes/SKILL.md").is_file());
    assert!(repo.join("skills/pdf/SKILL.md").is_file());
    assert_linked(&sandbox, "notes", "writing/notes");

    let twice = Sandbox::new();
    let folder = setup(&twice).folder;
    write(
        folder.join("skills/writing/notes/SKILL.md"),
        skill_md("notes"),
    );
    let output = import(&twice, &folder);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("skills/notes and skills/writing/notes, two skills named `notes`"),
        "{stderr}"
    );
}
