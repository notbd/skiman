//! Git hooks of the skill repo: `init` installs them, pre-commit checks the
//! staged snapshot, the post hooks distribute. Every hook here is a real git
//! hook fired by real git commands in the sandbox.

mod support;

use std::path::PathBuf;

use snapbox::cmd::Command;
use snapbox::str;
use support::{Sandbox, Tool, assert_symlink};

fn init(sandbox: &Sandbox) {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
}

/// git in the skill repo with a commit identity; the caller asserts the outcome.
fn git(sandbox: &Sandbox) -> Command {
    sandbox
        .command("git")
        .current_dir(sandbox.skill_repo())
        .args([
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=user@example.test",
        ])
}

fn write(sandbox: &Sandbox, rel: &str, contents: &str) -> PathBuf {
    let path = sandbox.skill_repo().join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents).unwrap();
    path
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Does {name} things\n---\n\n# {name}\n")
}

/// Stages everything and commits; asserts that git (and so pre-commit) succeeded.
fn commit_all(sandbox: &Sandbox, message: &str) {
    sandbox.git(sandbox.skill_repo(), ["add", "--all"]);
    git(sandbox)
        .args(["commit", "--quiet", "-m", message])
        .assert()
        .success();
}

fn link(sandbox: &Sandbox, tool: Tool, name: &str) -> PathBuf {
    sandbox.tool_dir(tool).join(name)
}

fn assert_linked(sandbox: &Sandbox, name: &str) {
    let target = sandbox.skill_repo().join("skills").join(name);
    for tool in Tool::ALL {
        assert_symlink(link(sandbox, tool, name), &target, &target);
    }
}

fn assert_not_linked(sandbox: &Sandbox, name: &str) {
    for tool in Tool::ALL {
        let path = link(sandbox, tool, name);
        assert!(
            std::fs::symlink_metadata(&path).is_err(),
            "{} still exists",
            path.display()
        );
    }
}

fn hooks_dir(sandbox: &Sandbox) -> PathBuf {
    sandbox.skill_repo().join(".git/hooks")
}

#[test]
fn committing_a_hand_created_skill_folder_links_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    assert_not_linked(&sandbox, "pdf");

    commit_all(&sandbox, "add pdf");

    assert_linked(&sandbox, "pdf");
}

#[test]
fn checking_out_a_branch_without_the_skill_removes_its_links() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    commit_all(&sandbox, "layout");
    sandbox.git(sandbox.skill_repo(), ["branch", "without-pdf"]);
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    commit_all(&sandbox, "add pdf");
    assert_linked(&sandbox, "pdf");

    git(&sandbox)
        .args(["checkout", "--quiet", "without-pdf"])
        .assert()
        .success();

    assert_not_linked(&sandbox, "pdf");
}

#[test]
fn a_merge_distributes_the_merged_skills() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    commit_all(&sandbox, "layout");
    let repo = sandbox.skill_repo();
    sandbox.git(&repo, ["switch", "--quiet", "-c", "feature"]);
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    commit_all(&sandbox, "add pdf");
    sandbox.git(&repo, ["switch", "--quiet", "-"]);
    write(&sandbox, "skills/docx/SKILL.md", &skill_md("docx"));
    commit_all(&sandbox, "add docx");
    assert_not_linked(&sandbox, "pdf");

    git(&sandbox)
        .args(["merge", "--quiet", "--no-edit", "feature"])
        .assert()
        .success();

    assert_linked(&sandbox, "pdf");
    assert_linked(&sandbox, "docx");
}

#[test]
fn a_rebase_distributes_the_rebased_skills() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    commit_all(&sandbox, "layout");
    let repo = sandbox.skill_repo();
    sandbox.git(&repo, ["switch", "--quiet", "-c", "feature"]);
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    commit_all(&sandbox, "add pdf");
    sandbox.git(&repo, ["switch", "--quiet", "-"]);
    write(&sandbox, "skills/docx/SKILL.md", &skill_md("docx"));
    commit_all(&sandbox, "add docx");
    sandbox.git(&repo, ["switch", "--quiet", "feature"]);
    assert_not_linked(&sandbox, "docx");

    git(&sandbox)
        .args(["rebase", "--quiet", "-"])
        .assert()
        .success();

    assert_linked(&sandbox, "pdf");
    assert_linked(&sandbox, "docx");
}

#[test]
fn every_post_hook_syncs_and_exits_0() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    commit_all(&sandbox, "add pdf");

    for hook in ["post-commit", "post-merge", "post-checkout", "post-rewrite"] {
        for tool in Tool::ALL {
            std::fs::remove_file(link(&sandbox, tool, "pdf")).unwrap();
        }
        git(&sandbox)
            .args(["hook", "run", hook])
            .assert()
            .code(0)
            .stdout_eq("");
        assert_linked(&sandbox, "pdf");
    }
}

const NO_DESCRIPTION: &str = "---\nname: pdf\n---\n\n# pdf\n";

#[test]
fn a_commit_whose_staged_snapshot_breaks_a_rule_is_rejected_though_the_working_tree_is_fixed() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    commit_all(&sandbox, "layout");
    let repo = sandbox.skill_repo();
    write(&sandbox, "skills/pdf/SKILL.md", NO_DESCRIPTION);
    sandbox.git(&repo, ["add", "--all"]);
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));

    git(&sandbox)
        .args(["commit", "--quiet", "-m", "add pdf"])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
skills/pdf/SKILL.md:[..]: required-field: [..]

"#]]);

    let no_commit = sandbox
        .std_command("git")
        .current_dir(&repo)
        .args(["rev-parse", "--verify", "--quiet", "HEAD~1"])
        .status()
        .unwrap();
    assert!(!no_commit.success(), "the commit must be rejected");
}

#[test]
fn a_commit_whose_staged_snapshot_is_valid_is_accepted_though_the_working_tree_is_broken() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    sandbox.git(&repo, ["add", "--all"]);
    write(&sandbox, "skills/pdf/SKILL.md", NO_DESCRIPTION);
    sandbox.skiman().arg("check").assert().code(1);

    git(&sandbox)
        .args(["commit", "--quiet", "-m", "add pdf"])
        .assert()
        .success();

    assert_eq!(
        sandbox.git(&repo, ["show", "HEAD:skills/pdf/SKILL.md"]) + "\n",
        skill_md("pdf")
    );
}

#[test]
fn a_commit_that_stages_a_skiman_conflict_marker_is_rejected() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    write(
        &sandbox,
        "skills/pdf/notes.md",
        "<<<<<<< local (skiman)\nmine\n=======\ntheirs\n>>>>>>> upstream (skiman)\n",
    );
    sandbox.git(sandbox.skill_repo(), ["add", "--all"]);

    git(&sandbox)
        .args(["commit", "--quiet", "-m", "add pdf"])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
skills/pdf/notes.md:1: unresolved-conflict: [..]
skills/pdf/notes.md:5: unresolved-conflict: [..]

"#]]);
}

#[test]
fn a_commit_of_named_paths_is_checked_as_it_will_be_recorded() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    commit_all(&sandbox, "layout");
    let repo = sandbox.skill_repo();
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    sandbox.git(&repo, ["add", "--all"]);
    write(&sandbox, "skills/pdf/SKILL.md", NO_DESCRIPTION);

    // `git commit <paths>` records the working tree version of the paths
    // through a temporary index; that is what pre-commit must check.
    git(&sandbox)
        .args([
            "commit",
            "--quiet",
            "-m",
            "add pdf",
            "--",
            "skills/pdf/SKILL.md",
        ])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
skills/pdf/SKILL.md:[..]: required-field: [..]

"#]]);
}

/// `init` through a symlink in the sandbox, so the recorded binary can be deleted.
fn init_through_removable_binary(sandbox: &Sandbox) -> PathBuf {
    let bin = sandbox.root().join("bin/skiman");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_skiman"), &bin).unwrap();
    sandbox
        .command(&bin)
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
    bin
}

#[test]
fn with_the_recorded_binary_missing_pre_commit_rejects_and_the_post_hooks_exit_0() {
    let sandbox = Sandbox::new();
    let bin = init_through_removable_binary(&sandbox);
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    sandbox.git(sandbox.skill_repo(), ["add", "--all"]);
    std::fs::remove_file(&bin).unwrap();

    git(&sandbox)
        .args(["commit", "--quiet", "-m", "add pdf"])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
skiman: [..]/bin/skiman does not exist, so the commit is rejected; install skiman, then run `skiman init <skill repo>` again

"#]]);

    for hook in ["post-commit", "post-merge", "post-checkout", "post-rewrite"] {
        git(&sandbox)
            .args(["hook", "run", hook])
            .assert()
            .code(0)
            .stdout_eq("")
            .stderr_eq(str![[r#"
skiman: [..]/bin/skiman does not exist, so the agent tools were not updated; install skiman, then run `skiman init <skill repo>` again

"#]]);
    }
}

#[test]
fn rerunning_init_replaces_skimans_own_hooks_without_duplicating_them() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let hooks = hooks_dir(&sandbox);
    let names = [
        "pre-commit",
        "post-commit",
        "post-merge",
        "post-checkout",
        "post-rewrite",
    ];
    let first: Vec<Vec<u8>> = names
        .iter()
        .map(|n| std::fs::read(hooks.join(n)).unwrap())
        .collect();
    // An edited skiman hook (the marker line kept) is skiman's and gets replaced.
    let mut edited = first[0].clone();
    edited.extend_from_slice(b"echo extra\n");
    std::fs::write(hooks.join("pre-commit"), edited).unwrap();

    init(&sandbox);

    for (name, before) in names.iter().zip(&first) {
        assert_eq!(&std::fs::read(hooks.join(name)).unwrap(), before, "{name}");
    }
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    commit_all(&sandbox, "add pdf");
    assert_linked(&sandbox, "pdf");
}

#[test]
fn init_leaves_a_pre_commit_hook_that_is_not_skimans_byte_identical_and_prints_the_line_to_add() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let pre_commit = hooks_dir(&sandbox).join("pre-commit");
    let foreign = b"#!/bin/sh\n# my own checks\nexit 0\n";
    std::fs::write(&pre_commit, foreign).unwrap();

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0)
        .stderr_eq(str![[r#"
warning: [..]/skill-repo/.git/hooks/pre-commit is not skiman's git hook, so skiman left it unchanged; add this line to it:
  '[..]/skiman' check --staged || exit 1

"#]]);

    assert_eq!(std::fs::read(&pre_commit).unwrap(), foreign);
}

#[test]
fn init_writes_no_hook_when_core_hooks_path_points_elsewhere_and_prints_the_lines_to_add() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let elsewhere = sandbox.root().join("shared-hooks");
    std::fs::create_dir(&elsewhere).unwrap();
    let foreign = b"#!/bin/sh\nexit 0\n";
    std::fs::write(elsewhere.join("pre-commit"), foreign).unwrap();
    sandbox.git(
        sandbox.skill_repo(),
        ["config", "core.hooksPath", elsewhere.to_str().unwrap()],
    );
    let own_hooks = support::tree(hooks_dir(&sandbox));

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq(str![[r#"
warning: core.hooksPath points to [..]/shared-hooks, so skiman installed no git hooks; add these lines to the hooks there:
  pre-commit:
    '[..]/skiman' check --staged || exit 1
  post-commit:
    '[..]/skiman' sync --hook || true
  post-merge:
    '[..]/skiman' sync --hook || true
  post-checkout:
    '[..]/skiman' sync --hook || true
  post-rewrite:
    '[..]/skiman' sync --hook || true

"#]]);

    let entries: Vec<_> = std::fs::read_dir(&elsewhere).unwrap().collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        std::fs::read(elsewhere.join("pre-commit")).unwrap(),
        foreign
    );
    assert_eq!(support::tree(hooks_dir(&sandbox)), own_hooks);
}

#[test]
fn check_staged_outside_a_git_repo_is_an_operational_error() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    std::fs::remove_dir_all(sandbox.skill_repo().join(".git")).unwrap();

    sandbox
        .skiman()
        .args(["check", "--staged"])
        .assert()
        .code(4)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: cannot read the staged snapshot of [..]/skill-repo: git ls-files failed: fatal: not a git repository[..]

"#]]);
}
