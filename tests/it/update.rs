//! `skiman update`: three-way merge of a new upstream version with the
//! user's local modifications (clean merges; conflicts are in
//! `update_conflicts.rs`).

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::support::{
    PDF_SKILL_MD, Sandbox, Upstream, backup_dirs, init, lines, read, tree, tree_hash, write,
};

/// Upstream `acme/pdf` (one skill at its root), installed as `pdf`.
fn installed(sandbox: &Sandbox) -> Upstream {
    init(sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", PDF_SKILL_MD)
        .write("reference.md", lines(&[]))
        .write("forms.md", lines(&[]));
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
    up
}

fn commit_skill_repo(sandbox: &Sandbox, message: &str) {
    sandbox.git(sandbox.skill_repo(), ["add", "--all"]);
    sandbox.git(
        sandbox.skill_repo(),
        [
            "-c",
            "user.name=User",
            "-c",
            "user.email=user@example.test",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    );
}

#[test]
fn a_clean_merge_keeps_committed_and_uncommitted_local_edits_and_applies_the_upstream_change() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    let skill = sandbox.skill_repo().join("skills/pdf");
    write(skill.join("reference.md"), lines(&[(2, "committed edit")]));
    commit_skill_repo(&sandbox, "local edit");
    write(
        skill.join("reference.md"),
        lines(&[(2, "committed edit"), (10, "uncommitted edit")]),
    );
    up.write("reference.md", lines(&[(18, "upstream fix")]));
    let commit = up.commit("v2");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {commit}; changed files:\n  modified reference.md\n\
             saved the old folders and records of pdf to [..]\n"
        ))
        .stderr_eq("");

    assert_eq!(
        read(skill.join("reference.md")),
        lines(&[
            (2, "committed edit"),
            (10, "uncommitted edit"),
            (18, "upstream fix")
        ])
    );
}

#[test]
fn structural_changes_on_both_sides_merge_cleanly() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", PDF_SKILL_MD)
        .write("up-delete.md", lines(&[(1, "up delete")]))
        .write("up-rename.md", lines(&[(1, "up rename")]))
        .write("my-delete.md", lines(&[(1, "my delete")]))
        .write("my-rename.md", lines(&[(1, "my rename")]));
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
    let skill = sandbox.skill_repo().join("skills/pdf");
    write(skill.join("mine.md"), "added by me\n");
    std::fs::remove_file(skill.join("my-delete.md")).unwrap();
    std::fs::create_dir(skill.join("docs")).unwrap();
    std::fs::rename(
        skill.join("my-rename.md"),
        skill.join("docs/renamed-by-me.md"),
    )
    .unwrap();
    // Each side also edits the file the other side renames.
    write(
        skill.join("up-rename.md"),
        lines(&[(1, "up rename"), (10, "my edit")]),
    );
    up.write("theirs.md", "added upstream\n")
        .remove("up-delete.md")
        .remove("up-rename.md")
        .write("ref/renamed-upstream.md", lines(&[(1, "up rename")]))
        .write(
            "my-rename.md",
            lines(&[(1, "my rename"), (20, "upstream edit")]),
        );
    let commit = up.commit("v2");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {commit}; changed files:\n\
             \x20 modified docs/renamed-by-me.md\n\
             \x20 added ref/renamed-upstream.md\n\
             \x20 added theirs.md\n\
             \x20 deleted up-delete.md\n\
             \x20 deleted up-rename.md\n\
             saved the old folders and records of pdf to [..]\n"
        ))
        .stderr_eq("");

    assert_eq!(
        tree(&skill),
        BTreeMap::from([
            ("SKILL.md".into(), format!("file 644 {PDF_SKILL_MD}")),
            ("docs".into(), "dir".into()),
            (
                "docs/renamed-by-me.md".into(),
                format!(
                    "file 644 {}",
                    lines(&[(1, "my rename"), (20, "upstream edit")])
                )
            ),
            ("mine.md".into(), "file 644 added by me\n".into()),
            ("ref".into(), "dir".into()),
            (
                "ref/renamed-upstream.md".into(),
                format!("file 644 {}", lines(&[(1, "up rename"), (10, "my edit")]))
            ),
            ("theirs.md".into(), "file 644 added upstream\n".into()),
        ])
    );
}

#[test]
fn after_an_update_the_snapshot_and_lock_match_upstream_and_the_old_state_sits_in_one_backup() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    let repo = sandbox.skill_repo();
    let skill = repo.join("skills/pdf");
    write(skill.join("forms.md"), lines(&[(1, "uncommitted edit")]));
    let old_skill = tree(&skill);
    let old_upstream = tree(repo.join("upstream/pdf"));
    let old_manifest = read(repo.join("skiman.toml"));
    let old_lock = read(repo.join("skiman.lock"));
    up.write("reference.md", lines(&[(20, "upstream fix")]))
        .write_executable("run.sh", "#!/bin/sh\n");
    let commit = up.commit("v2");

    sandbox.skiman().args(["update", "pdf"]).assert().code(0);

    assert_eq!(tree(repo.join("upstream/pdf")), tree_at_head(&up));
    assert_eq!(
        read(repo.join("skiman.lock")),
        format!(
            "version = 1\n\n[skills.pdf]\ncommit = \"{commit}\"\nsnapshot = \"{}\"\n",
            tree_hash(&repo.join("upstream/pdf"))
        )
    );
    assert_eq!(read(repo.join("skiman.toml")), old_manifest);

    let dirs = backup_dirs(&sandbox);
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    let saved = dirs[0].join("repo");
    assert_eq!(tree(saved.join("skills/pdf")), old_skill);
    assert_eq!(tree(saved.join("upstream/pdf")), old_upstream);
    assert_eq!(
        read(saved.join("skiman.toml.pdf.fragment")),
        "[skills.pdf.source]\nurl = \"https://github.com/acme/pdf.git\"\npath = \"\"\n"
    );
    assert_eq!(
        read(saved.join("skiman.lock.pdf.fragment")),
        old_lock.strip_prefix("version = 1\n\n").unwrap()
    );
    assert_eq!(
        std::fs::read_dir(&saved).unwrap().count(),
        4,
        "only the two folders and two fragments"
    );
    assert!(
        !repo.join(".skiman/tmp").exists()
            || std::fs::read_dir(repo.join(".skiman/tmp")).unwrap().count() == 0,
        "no tmp leftovers"
    );
}

/// The upstream's working tree as `tree()` shows it, without `.git`.
fn tree_at_head(up: &Upstream) -> BTreeMap<PathBuf, String> {
    let mut all = tree(up.path());
    all.retain(|path, _| !path.starts_with(".git"));
    all
}

/// The whole skill repo (including `.git` and `.skiman`).
fn skill_repo_tree(sandbox: &Sandbox) -> BTreeMap<PathBuf, String> {
    tree(sandbox.skill_repo())
}

#[test]
fn an_up_to_date_skill_changes_nothing_and_creates_no_backup() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    write(
        sandbox.skill_repo().join("skills/pdf/forms.md"),
        "local edit\n",
    );
    let before = skill_repo_tree(&sandbox);

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!("skill pdf: up to date at {}\n", up.head()))
        .stderr_eq("");

    assert_eq!(skill_repo_tree(&sandbox), before);
    assert!(backup_dirs(&sandbox).is_empty());
}

fn inode(path: impl AsRef<Path>) -> u64 {
    std::fs::metadata(path).unwrap().ino()
}

#[test]
fn an_upstream_commit_that_does_not_touch_the_skill_advances_only_the_lock_commit() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    let repo = sandbox.skill_repo();
    let lock_before = read(repo.join("skiman.lock"));
    let old_commit = up.head();
    let inodes = |repo: &Path| {
        [
            inode(repo.join("skills/pdf")),
            inode(repo.join("skills/pdf/reference.md")),
            inode(repo.join("upstream/pdf")),
            inode(repo.join("upstream/pdf/reference.md")),
        ]
    };
    let before = inodes(&repo);
    let skill_before = tree(repo.join("skills/pdf"));
    let upstream_before = tree(repo.join("upstream/pdf"));
    let commit = up.commit("an empty commit");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: now at {commit}; no file of the skill changed\n\
             saved the records of pdf to [..]\n"
        ))
        .stderr_eq("");

    assert_eq!(inodes(&repo), before);
    assert_eq!(tree(repo.join("skills/pdf")), skill_before);
    assert_eq!(tree(repo.join("upstream/pdf")), upstream_before);
    assert_eq!(
        read(repo.join("skiman.lock")),
        lock_before.replace(&old_commit, &commit)
    );
    let dirs = backup_dirs(&sandbox);
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    assert_eq!(
        read(dirs[0].join("repo/skiman.lock.pdf.fragment")),
        lock_before.strip_prefix("version = 1\n\n").unwrap()
    );
    assert!(dirs[0].join("repo/skiman.toml.pdf.fragment").is_file());
}

#[test]
fn a_multi_skill_update_reports_each_skill_and_exits_with_the_highest_result() {
    let sandbox = Sandbox::new();
    let pdf = installed(&sandbox);
    let [docx, xlsx] = ["docx", "xlsx"].map(|name| {
        let up = sandbox.upstream(&format!("acme/{name}"));
        up.write(
            "SKILL.md",
            format!("---\nname: {name}\ndescription: Edit {name} files\n---\n"),
        );
        up.commit("v1");
        sandbox
            .skiman()
            .args(["add", &format!("acme/{name}"), "--yes"])
            .assert()
            .code(0);
        up
    });
    std::fs::remove_dir_all(docx.path()).unwrap();
    pdf.write("reference.md", lines(&[(5, "upstream fix")]));
    let commit = pdf.commit("v2");
    xlsx.write("reference.md", "# xlsx\n");
    let xlsx_commit = xlsx.commit("v2");

    sandbox
        .skiman()
        .arg("update")
        .assert()
        .code(4)
        .stdout_eq(format!(
            "skill pdf: updated to {commit}; changed files:\n  modified reference.md\n\
             saved the old folders and records of pdf to [..]\n\
             skill xlsx: updated to {xlsx_commit}; changed files:\n  added reference.md\n\
             saved the old folders and records of xlsx to [..]\n"
        ))
        .stderr_eq(
            "error: skill docx: reading the default branch of https://github.com/acme/docx.git failed: [..]\n...",
        );

    assert_eq!(
        read(sandbox.skill_repo().join("skills/pdf/reference.md")),
        lines(&[(5, "upstream fix")])
    );
    // Each skill's records are written from the files as they are then,
    // so the second does not put back the first one's old lock entry.
    let lock = read(sandbox.skill_repo().join("skiman.lock"));
    for (name, commit) in [("pdf", &commit), ("xlsx", &xlsx_commit)] {
        assert!(
            lock.contains(&format!("[skills.{name}]\ncommit = \"{commit}\"\n")),
            "{lock}"
        );
    }
    sandbox.skiman().arg("check").assert().code(0);
}

#[test]
fn the_merge_ignores_the_users_git_config_drivers_and_hooks() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    let skill = sandbox.skill_repo().join("skills/pdf");
    write(skill.join("reference.md"), lines(&[(2, "local edit")]));
    up.write("reference.md", lines(&[(18, "upstream fix")]));
    up.commit("v2");

    let marker = sandbox.root().join("marker");
    let hooks = sandbox.root().join("evil-hooks");
    for hook in [
        "pre-commit",
        "post-commit",
        "reference-transaction",
        "post-checkout",
        "post-merge",
        "post-index-change",
    ] {
        write(
            hooks.join(hook),
            format!("#!/bin/sh\necho {hook} >> '{}'\n", marker.display()),
        );
        std::fs::set_permissions(hooks.join(hook), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let attributes = sandbox.root().join("evil-attributes");
    write(&attributes, "* merge=evil filter=evil\n");
    let touch = format!("sh -c 'echo $0 >> {}'", marker.display());
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(sandbox.git_config())
        .unwrap();
    writeln!(
        config,
        "[core]\n\thooksPath = {}\n\tattributesFile = {}\n\
         [merge \"evil\"]\n\tdriver = {touch} merge-driver\n\
         [filter \"evil\"]\n\tclean = {touch} clean\n\tsmudge = {touch} smudge\n\
         [merge]\n\tconflictStyle = diff3",
        hooks.display(),
        attributes.display()
    )
    .unwrap();

    sandbox.skiman().args(["update", "pdf"]).assert().code(0);

    assert!(
        !marker.exists(),
        "something ran: {}",
        std::fs::read_to_string(&marker).unwrap_or_default()
    );
    assert_eq!(
        read(skill.join("reference.md")),
        lines(&[(2, "local edit"), (18, "upstream fix")])
    );
}

#[test]
fn a_rule_violation_in_a_file_the_merge_did_not_change_is_only_a_warning() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    let skill = sandbox.skill_repo().join("skills/pdf");
    write(skill.join("SKILL.md"), "---\nname: pdf\n---\n\n# pdf\n");
    up.write("reference.md", lines(&[(5, "upstream fix")]));
    up.commit("v2");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stderr_eq("warning: skills/pdf/SKILL.md:[..]: required-field: [..]\n");

    assert_eq!(
        read(skill.join("reference.md")),
        lines(&[(5, "upstream fix")])
    );
    assert_eq!(
        read(skill.join("SKILL.md")),
        "---\nname: pdf\n---\n\n# pdf\n"
    );
}

#[test]
fn a_rule_violation_in_a_file_the_merge_changed_refuses_the_update() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    up.write("SKILL.md", "---\nname: pdf\n---\n\n# pdf\n");
    up.commit("v2");
    let before = skill_repo_tree(&sandbox);

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(
            "error: skill pdf: refusing to write content that breaks the rules:\n  skills/pdf/SKILL.md:[..]: required-field: [..]\n",
        );

    assert_eq!(skill_repo_tree(&sandbox), before);
    assert!(backup_dirs(&sandbox).is_empty());
}

#[test]
fn a_secret_in_a_local_file_the_merge_did_not_change_refuses_the_update() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    let key = format!("AKIA{}", "ABCDEFGHIJKLMNOP");
    write(
        sandbox.skill_repo().join("skills/pdf/notes.md"),
        format!("key {key}\n"),
    );
    up.write("reference.md", lines(&[(5, "upstream fix")]));
    up.commit("v2");
    let before = skill_repo_tree(&sandbox);

    let assert = sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stderr_eq(
            "error: skill pdf: refusing to write content that breaks the rules:\n  skills/pdf/notes.md:1: aws-access-key: [..]\n...",
        );
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(!stderr.contains(&key));
    assert!(
        stderr.contains("path = \"skills/pdf/notes.md\""),
        "{stderr}"
    );
    assert_eq!(skill_repo_tree(&sandbox), before);
}

#[test]
fn only_third_party_skills_are_updated() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "mine", "-d", "Mine"])
        .assert()
        .code(0);

    sandbox
        .skiman()
        .arg("update")
        .assert()
        .code(0)
        .stdout_eq("no third-party skills to update\n");
    sandbox
        .skiman()
        .args(["update", "mine", "nope"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(
            "error: skill mine: skills/mine has no [skills.mine.source] table in skiman.toml, so it is an own skill; only third-party skills are updated\n\
             error: skill nope: there is no skill named nope\n",
        );
}

/// Both records are read leniently since #21 (`list` shows a damaged table);
/// `update` refuses a source table without `url` and a lock entry without
/// `snapshot`, before fetching.
#[test]
fn a_source_without_url_or_a_lock_entry_without_snapshot_is_refused() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    up.write("forms.md", lines(&[(5, "theirs")]));
    up.commit("v2");
    let repo = sandbox.skill_repo();
    let manifest = read(repo.join("skiman.toml"));
    let lock = read(repo.join("skiman.lock"));

    let without = |text: &str, key: &str| -> String {
        let kept: Vec<&str> = text
            .lines()
            .filter(|l| !l.starts_with(&format!("{key} =")))
            .collect();
        assert_ne!(kept.len(), text.lines().count(), "no `{key}` in:\n{text}");
        kept.join("\n") + "\n"
    };
    write(repo.join("skiman.toml"), without(&manifest, "url"));
    let before = skill_repo_tree(&sandbox);
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(
            "error: skill pdf: [skills.pdf.source] in skiman.toml has no `url`; run `skiman check`\n",
        );
    assert_eq!(skill_repo_tree(&sandbox), before);

    write(repo.join("skiman.toml"), &manifest);
    write(repo.join("skiman.lock"), without(&lock, "snapshot"));
    let before = skill_repo_tree(&sandbox);
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq("error: skill pdf: pdf has no entry in skiman.lock; run `skiman check`\n");
    assert_eq!(skill_repo_tree(&sandbox), before);
}

/// Spec Source fetching: a `url` in skiman.toml (a cloned skill repo or a
/// hand edit) gets the same check as `add`'s source, before git runs on it.
#[test]
fn a_remote_helper_url_in_skiman_toml_is_refused_before_git_runs() {
    let sandbox = Sandbox::new();
    installed(&sandbox);
    let repo = sandbox.skill_repo();
    let bin = sandbox.root().join("bin");
    let pwned = sandbox.home().join("PWNED");
    write(
        bin.join("git-remote-evil"),
        format!("#!/bin/sh\ntouch '{}'\n", pwned.display()),
    );
    std::fs::set_permissions(
        bin.join("git-remote-evil"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    for url in ["evil::whatever", "file:///tmp/repo", "git://example.test/x"] {
        let manifest = read(repo.join("skiman.toml"));
        let line = manifest
            .lines()
            .find(|l| l.starts_with("url = "))
            .expect("a url line")
            .to_owned();
        write(
            repo.join("skiman.toml"),
            manifest.replace(&line, &format!("url = \"{url}\"")),
        );
        let before = skill_repo_tree(&sandbox);
        sandbox
            .skiman()
            .env("PATH", &path)
            .args(["update", "pdf"])
            .assert()
            .code(1)
            .stdout_eq("")
            .stderr_eq(format!(
                "error: skill pdf: [skills.pdf.source] in skiman.toml: the `url` `{url}` [..]; nothing was fetched\n"
            ));
        assert_eq!(skill_repo_tree(&sandbox), before);
        assert!(!pwned.exists(), "the remote helper ran for {url}");
    }
}

/// Story 19: the install name stays when upstream renames its frontmatter
/// `name`; the `name` line is a local modification and the new upstream
/// name is recorded.
#[test]
fn an_upstream_rename_of_the_frontmatter_name_keeps_the_install_name() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    let repo = sandbox.skill_repo();
    up.write(
        "SKILL.md",
        PDF_SKILL_MD.replace("name: pdf\n", "name: pdf-tools\n"),
    )
    .write("forms.md", lines(&[(5, "theirs")]));
    let commit = up.commit("v2");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {commit}; changed files:\n  modified forms.md\n\
             saved the old folders and records of pdf to [..]\n"
        ))
        .stderr_eq("");

    assert_eq!(read(repo.join("skills/pdf/SKILL.md")), PDF_SKILL_MD);
    sandbox.skiman().args(["check"]).assert().code(0);
}

/// Clarification 3: tmp leftovers move before the command's first write,
/// even when that write only advances a skill's records.
#[test]
fn tmp_leftovers_move_before_a_records_only_advance() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    let tmp = sandbox.skill_repo().join(".skiman/tmp");
    write(tmp.join("20260101T000000Z/stray.md"), "left behind\n");
    let commit = up.commit("an empty commit");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "moved tmp leftovers to [..]/repo/.skiman/tmp\n\
             skill pdf: now at {commit}; no file of the skill changed\n\
             saved the records of pdf to [..]\n"
        ))
        .stderr_eq("");
    assert!(!tmp.join("20260101T000000Z").exists());
}

/// `pdf` from an upstream whose root holds a `.gitignore`, laid out the way
/// skiman installed it before it left such files out: the file in
/// `skills/pdf` and `upstream/pdf`, and in the lock's snapshot hash.
fn installed_with_a_gitignore(sandbox: &Sandbox) -> Upstream {
    init(sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", PDF_SKILL_MD)
        .write("reference.md", lines(&[]))
        .write(".gitignore", "*.log\n");
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
    let repo = sandbox.skill_repo();
    for dir in ["skills/pdf", "upstream/pdf"] {
        write(repo.join(dir).join(".gitignore"), "*.log\n");
    }
    let lock = read(repo.join("skiman.lock"));
    let snapshot = lock
        .lines()
        .find_map(|line| line.strip_prefix("snapshot = "))
        .unwrap();
    let lock = lock.replace(
        snapshot,
        &format!("\"{}\"", tree_hash(&repo.join("upstream/pdf"))),
    );
    std::fs::write(repo.join("skiman.lock"), lock).unwrap();
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
    up
}

#[test]
fn an_unchanged_upstream_gitignore_from_an_earlier_install_is_removed_by_the_next_update() {
    let sandbox = Sandbox::new();
    let up = installed_with_a_gitignore(&sandbox);
    let commit = up.head();

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {commit}; changed files:\n  deleted .gitignore\n\
             \x20 .gitignore  [not installed]\n\
             saved the old folders and records of pdf to [..]\n"
        ))
        .stderr_eq("");

    let repo = sandbox.skill_repo();
    assert!(!repo.join("skills/pdf/.gitignore").exists());
    assert!(!repo.join("upstream/pdf/.gitignore").exists());
    let snapshot = tree_hash(&repo.join("upstream/pdf"));
    assert!(
        read(repo.join("skiman.lock")).contains(&format!("snapshot = \"{snapshot}\"")),
        "the lock does not hold {snapshot}"
    );
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: up to date at {commit}\n  .gitignore  [not installed]\n"
        ))
        .stderr_eq("");
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

/// Spec Source fetching: submodules are not fetched and are listed in the
/// summary, by `update` as by `add`.
#[test]
fn update_lists_the_submodules_it_does_not_fetch() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    let head = up.head();
    up.write("forms.md", lines(&[(5, "theirs")]));
    up.git(["add", "--all"]);
    up.git([
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{head},vendor/lib"),
    ]);
    up.git(["commit", "--quiet", "-m", "v2"]);
    let commit = up.head();

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {commit}; changed files:\n  modified forms.md\n\
             \x20 vendor/lib  [submodule, not fetched]\n\
             saved the old folders and records of pdf to [..]\n"
        ))
        .stderr_eq("");
    assert!(!sandbox.skill_repo().join("skills/pdf/vendor").exists());
}

#[test]
fn an_upstream_that_no_longer_has_the_skill_folder_is_refused_naming_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/skills");
    up.write("skills/pdf/SKILL.md", PDF_SKILL_MD)
        .write("README.md", "# Skills\n");
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/skills", "--yes"])
        .assert()
        .code(0);
    // A hand-written `path` may end in `/`, as `check` allows.
    let manifest = sandbox.skill_repo().join("skiman.toml");
    let text = read(&manifest);
    assert!(text.contains("path = \"skills/pdf\"\n"));
    write(&manifest, text.replace("\"skills/pdf\"", "\"skills/pdf/\""));
    up.write("README.md", "# Skills, v2\n");
    up.commit("v2");
    sandbox.skiman().args(["update", "pdf"]).assert().code(0);
    up.remove("skills/pdf");
    let commit = up.commit("v3");
    let before = tree(sandbox.skill_repo());

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(format!(
            "error: skill pdf: https://github.com/acme/skills.git has no folder `skills/pdf/` at {commit}\n"
        ));
    assert_eq!(tree(sandbox.skill_repo()), before);
}

#[test]
fn a_dry_run_shows_the_new_version_and_the_changed_files_and_writes_nothing() {
    let sandbox = Sandbox::new();
    let up = installed(&sandbox);
    up.write("forms.md", lines(&[(5, "theirs")]))
        .write_executable("reference.md", lines(&[]))
        .write_executable("scripts/fill.sh", "#!/bin/sh\n");
    let commit = up.commit("v2");
    let files = "  modified forms.md\n  modified reference.md  [executable]\n  added scripts/fill.sh  [script, executable]\n";
    let before = tree(sandbox.skill_repo());
    // A dry run writes nothing, so it does not need the repo lock.
    let lock = std::fs::File::open(sandbox.skill_repo().join(".skiman/lock")).unwrap();
    lock.lock().unwrap();

    sandbox
        .skiman()
        .args(["update", "--dry-run"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: would update to {commit}; changed files:\n{files}dry run: nothing was written\n"
        ))
        .stderr_eq("");
    assert_eq!(tree(sandbox.skill_repo()), before);
    drop(lock);

    sandbox
        .skiman()
        .args(["update"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {commit}; changed files:\n{files}saved the old folders and records of pdf to [..]\n"
        ));
}
