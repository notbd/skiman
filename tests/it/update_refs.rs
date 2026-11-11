//! `skiman update` and versions (spec Version semantics): a followed branch
//! moves with its head, a pinned tag or commit stays at the locked commit,
//! `--ref` and a hand-edited `source.ref` change the version, and local
//! folders are re-read.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::support::{Sandbox, Upstream, tree};

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

const PDF_SKILL_MD: &str = "---\nname: pdf\ndescription: Fill PDF forms\n---\n\n# pdf\n";

/// `acme/pdf` (one skill at its root) with `VERSION` = 1 at the first commit.
fn upstream(sandbox: &Sandbox) -> (Upstream, String) {
    init(sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", PDF_SKILL_MD).write("VERSION", "1\n");
    let one = up.commit("one");
    (up, one)
}

fn add(sandbox: &Sandbox, extra: &[&str]) {
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .args(extra)
        .assert()
        .code(0);
}

fn repo(sandbox: &Sandbox) -> PathBuf {
    sandbox.skill_repo()
}

fn version(sandbox: &Sandbox) -> String {
    read(repo(sandbox).join("skills/pdf/VERSION"))
}

fn source_table(git_ref: Option<(&str, &str)>) -> String {
    let refs = git_ref.map_or_else(String::new, |(r, kind)| {
        format!("ref = \"{r}\"\nref_kind = \"{kind}\"\n")
    });
    format!("[skills.pdf.source]\nurl = \"https://github.com/acme/pdf.git\"\npath = \"\"\n{refs}")
}

fn lock_entry(commit: &str, git_ref: Option<&str>) -> String {
    let r = git_ref.map_or_else(String::new, |r| format!("ref = \"{r}\"\n"));
    format!("[skills.pdf]\ncommit = \"{commit}\"\n{r}snapshot = ")
}

fn assert_records(sandbox: &Sandbox, commit: &str, git_ref: Option<(&str, &str)>) {
    let toml = read(repo(sandbox).join("skiman.toml"));
    assert!(toml.ends_with(&source_table(git_ref)), "{toml}");
    let lock = read(repo(sandbox).join("skiman.lock"));
    assert!(
        lock.contains(&lock_entry(commit, git_ref.map(|(r, _)| r))),
        "{lock}"
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

fn edit_manifest(sandbox: &Sandbox, from: &str, to: &str) {
    let path = repo(sandbox).join("skiman.toml");
    let text = read(&path);
    assert!(text.contains(from), "{text}");
    write(&path, &text.replace(from, to));
}

fn inodes(dir: impl AsRef<Path>) -> BTreeMap<PathBuf, u64> {
    tree(dir.as_ref())
        .into_keys()
        .map(|p| {
            let ino = std::fs::symlink_metadata(dir.as_ref().join(&p))
                .unwrap()
                .ino();
            (p, ino)
        })
        .collect()
}

#[test]
fn a_branch_following_skill_picks_up_the_new_head() {
    let sandbox = Sandbox::new();
    let (up, _) = upstream(&sandbox);
    up.switch("stable");
    add(&sandbox, &["--ref", "stable"]);
    up.write("VERSION", "2\n");
    let two = up.commit("two");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {two}; changed files:\n  modified VERSION\n\
             saved the old folders and records of pdf to [..]\n"
        ))
        .stderr_eq("");

    assert_eq!(version(&sandbox), "2\n");
    assert_records(&sandbox, &two, Some(("stable", "branch")));
}

#[test]
fn a_tag_pinned_skill_stays_at_its_locked_commit_after_the_tag_moves_and_reports_it() {
    let sandbox = Sandbox::new();
    let (up, one) = upstream(&sandbox);
    up.tag("v1");
    add(&sandbox, &["--ref", "v1"]);
    up.write("VERSION", "2\n");
    let two = up.commit("two");
    up.tag("v1");
    let before = tree(repo(&sandbox));

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: up to date at {one}\n\
             \x20 note: tag `v1` now points to {two}; pdf stays at its locked commit, \
             and `skiman update pdf --ref v1` moves it there\n"
        ))
        .stderr_eq("");

    assert_eq!(tree(repo(&sandbox)), before);
    assert_records(&sandbox, &one, Some(("v1", "tag")));
}

#[test]
fn a_commit_pinned_skill_stays_at_its_commit_when_the_default_branch_moves() {
    let sandbox = Sandbox::new();
    let (up, one) = upstream(&sandbox);
    add(&sandbox, &["--ref", &one]);
    up.write("VERSION", "2\n");
    up.commit("two");
    let before = tree(repo(&sandbox));

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!("skill pdf: up to date at {one}\n"))
        .stderr_eq("");

    assert_eq!(tree(repo(&sandbox)), before);
    assert_records(&sandbox, &one, Some((&one, "commit")));
}

#[test]
fn update_ref_writes_source_ref_ref_kind_and_the_lock_ref_together() {
    let sandbox = Sandbox::new();
    let (up, _) = upstream(&sandbox);
    add(&sandbox, &[]);
    up.write("VERSION", "2\n");
    let two = up.commit("two");
    up.tag("v2");
    up.write("VERSION", "3\n");
    up.commit("three");

    sandbox
        .skiman()
        .args(["update", "pdf", "--ref", "v2"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {two}; changed files:\n  modified VERSION\n\
             saved the old folders and records of pdf to [..]\n"
        ))
        .stderr_eq("");

    assert_eq!(version(&sandbox), "2\n");
    assert_records(&sandbox, &two, Some(("v2", "tag")));
}

#[test]
fn update_ref_with_two_names_or_none_is_a_usage_error() {
    let sandbox = Sandbox::new();
    let (up, _) = upstream(&sandbox);
    add(&sandbox, &[]);
    up.tag("v1");
    let before = tree(repo(&sandbox));

    for args in [
        &["update", "pdf", "docx", "--ref", "v1"][..],
        &["update", "--ref", "v1"][..],
    ] {
        sandbox
            .skiman()
            .args(args)
            .assert()
            .code(2)
            .stdout_eq("")
            .stderr_eq("error: --ref needs exactly one skill name\n");
    }
    assert_eq!(tree(repo(&sandbox)), before);
}

#[test]
fn a_hand_edited_source_ref_makes_the_next_update_behave_as_update_ref() {
    let sandbox = Sandbox::new();
    let (up, _) = upstream(&sandbox);
    add(&sandbox, &[]);
    up.write("VERSION", "2\n");
    let two = up.commit("two");
    up.tag("v2");
    up.write("VERSION", "3\n");
    up.commit("three");
    // `ref_kind` may be omitted by hand.
    edit_manifest(&sandbox, "path = \"\"\n", "path = \"\"\nref = \"v2\"\n");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {two}; changed files:\n  modified VERSION\n\
             saved the old folders and records of pdf to [..]\n"
        ));

    assert_eq!(version(&sandbox), "2\n");
    assert_records(&sandbox, &two, Some(("v2", "tag")));
}

#[test]
fn removing_source_ref_by_hand_follows_the_default_branch_and_removes_the_lock_ref() {
    let sandbox = Sandbox::new();
    let (up, _) = upstream(&sandbox);
    up.tag("v1");
    add(&sandbox, &["--ref", "v1"]);
    up.write("VERSION", "2\n");
    let two = up.commit("two");
    // `ref_kind` left behind is removed too: it is never trusted.
    edit_manifest(&sandbox, "ref = \"v1\"\n", "");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {two}; changed files:\n  modified VERSION\n\
             saved the old folders and records of pdf to [..]\n"
        ));

    assert_eq!(version(&sandbox), "2\n");
    assert_records(&sandbox, &two, None);
}

#[test]
fn update_ref_to_a_tag_at_the_locked_commit_rewrites_only_the_metadata() {
    let sandbox = Sandbox::new();
    let (up, one) = upstream(&sandbox);
    add(&sandbox, &[]);
    up.tag("v1");
    let skill = inodes(repo(&sandbox).join("skills/pdf"));
    let snapshot = inodes(repo(&sandbox).join("upstream/pdf"));
    let skill_tree = tree(repo(&sandbox).join("skills/pdf"));
    let old_manifest = read(repo(&sandbox).join("skiman.toml"));

    sandbox
        .skiman()
        .args(["update", "pdf", "--ref", "v1"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: now at {one}; no file of the skill changed\n\
             saved the records of pdf to [..]\n"
        ))
        .stderr_eq("");

    assert_eq!(inodes(repo(&sandbox).join("skills/pdf")), skill);
    assert_eq!(inodes(repo(&sandbox).join("upstream/pdf")), snapshot);
    assert_eq!(tree(repo(&sandbox).join("skills/pdf")), skill_tree);
    assert_records(&sandbox, &one, Some(("v1", "tag")));
    let backups: Vec<_> = std::fs::read_dir(sandbox.state_home().join("skiman/backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        read(backups[0].join("repo/skiman.toml.pdf.fragment")),
        old_manifest[old_manifest.find("[skills.pdf.source]").unwrap()..]
    );
}

#[test]
fn a_wrong_hand_written_ref_kind_is_corrected() {
    let sandbox = Sandbox::new();
    let (up, one) = upstream(&sandbox);
    up.tag("v1");
    add(&sandbox, &["--ref", "v1"]);
    edit_manifest(&sandbox, "ref_kind = \"tag\"", "ref_kind = \"branch\"");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: now at {one}; no file of the skill changed\n\
             saved the records of pdf to [..]\n"
        ));

    assert_records(&sandbox, &one, Some(("v1", "tag")));
}

/// `~/src/pdf` holding one skill, installed as `pdf`.
fn local_installed(sandbox: &Sandbox) -> PathBuf {
    init(sandbox);
    let dir = sandbox.home().join("src/pdf");
    write(dir.join("SKILL.md"), PDF_SKILL_MD);
    write(dir.join("VERSION"), "1\n");
    sandbox
        .skiman()
        .args(["add", "~/src/pdf", "--yes"])
        .assert()
        .code(0);
    dir
}

#[test]
fn a_local_folder_source_picks_up_changed_files() {
    let sandbox = Sandbox::new();
    let dir = local_installed(&sandbox);
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq("skill pdf: up to date at sha256:[..]\n");
    write(dir.join("VERSION"), "2\n");
    write(dir.join(".git/config"), "not part of the skill\n");

    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(0)
        .stdout_eq(
            "skill pdf: updated to sha256:[..]; changed files:\n  modified VERSION\n\
             saved the old folders and records of pdf to [..]\n",
        )
        .stderr_eq("");

    assert_eq!(version(&sandbox), "2\n");
    assert!(!repo(&sandbox).join("upstream/pdf/.git").exists());
    let lock = read(repo(&sandbox).join("skiman.lock"));
    assert!(
        lock.contains("[skills.pdf]\nsnapshot = \"sha256:"),
        "{lock}"
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn a_local_folder_source_refuses_ref_and_a_missing_folder_keeps_the_current_content() {
    let sandbox = Sandbox::new();
    let dir = local_installed(&sandbox);
    let before = tree(repo(&sandbox));

    sandbox
        .skiman()
        .args(["update", "pdf", "--ref", "main"])
        .assert()
        .code(1)
        .stderr_eq(
            "error: skill pdf: ~/src/pdf is a local folder, which has no versions; drop --ref\n",
        );
    assert_eq!(tree(repo(&sandbox)), before);

    std::fs::rename(&dir, sandbox.home().join("src/moved")).unwrap();
    sandbox
        .skiman()
        .args(["update", "pdf"])
        .assert()
        .code(4)
        .stderr_eq(format!(
            "error: skill pdf: the local folder {} does not exist; nothing of pdf changed\n",
            dir.display()
        ));
    assert_eq!(tree(repo(&sandbox)), before);
}

#[test]
fn update_ref_to_a_name_that_is_both_a_branch_and_a_tag_follows_the_branch_and_reports_the_tag() {
    let sandbox = Sandbox::new();
    let (up, one) = upstream(&sandbox);
    add(&sandbox, &[]);
    up.git(["tag", "release", &one]);
    up.write("VERSION", "2\n");
    let two = up.commit("two");
    up.branch("release");

    sandbox
        .skiman()
        .args(["update", "pdf", "--ref", "release"])
        .assert()
        .code(0)
        .stdout_eq(format!(
            "skill pdf: updated to {two}; changed files:\n  modified VERSION\n\
             saved the old folders and records of pdf to [..]\n\
             \x20 note: `release` is also a tag; the branch is followed\n"
        ));

    assert_records(&sandbox, &two, Some(("release", "branch")));
}
