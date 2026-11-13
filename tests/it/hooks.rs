//! Git hooks of the skill repo: `init` installs them, pre-commit checks the
//! staged snapshot, the post hooks distribute. Every hook here is a real git
//! hook fired by real git commands in the sandbox.

use std::path::{Path, PathBuf};

use snapbox::cmd::Command;
use snapbox::str;

use crate::support::{PDF_SKILL_MD, Sandbox, Tool, Upstream, assert_symlink, init, skill_md};

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

    let output = git(&sandbox)
        .args(["commit", "--quiet", "-m", "add pdf"])
        .assert()
        .code(1)
        .stderr_eq(str![[r#"
skills/pdf/SKILL.md:[..]: required-field: [..]

The report above is about the staged snapshot (the git index), not about your files: the working tree passes every rule. Stage these paths with this line, then commit again:

  git -C '[..]/skill-repo' add -A -- skiman.toml skills instructions

It stages every change under these paths; to commit only some of them, stage those yourself. If the commit is still rejected, read the new report. Common causes:
  - an ignore rule hides a file: see `git -C '[..]/skill-repo' status --ignored -- skiman.toml skills instructions`
  - a skill folder holds its own .git, so git records it as a single entry of mode 160000: see `git -C '[..]/skill-repo' ls-files -s -- skiman.toml skills instructions`
  - line-ending conversion or a filter changed the content (core.autocrlf, .gitattributes)

"#]])
        .get_output()
        .clone();

    let no_commit = sandbox
        .std_command("git")
        .current_dir(&repo)
        .args(["rev-parse", "--verify", "--quiet", "HEAD~1"])
        .status()
        .unwrap();
    assert!(!no_commit.success(), "the commit must be rejected");

    stage_as_printed_and_commit(&sandbox, &String::from_utf8(output.stderr).unwrap(), &repo);
    assert_eq!(
        sandbox.git(&repo, ["show", "HEAD:skills/pdf/SKILL.md"]) + "\n",
        skill_md("pdf")
    );
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
    let own_hooks = crate::support::tree(hooks_dir(&sandbox));

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
    assert_eq!(crate::support::tree(hooks_dir(&sandbox)), own_hooks);
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

/// After `init` switches to another skill repo, the previous repo keeps
/// skiman's hooks; its commits are not checked against the new skill repo.
/// (git sends a hook's stdout to stderr.)
#[test]
fn a_commit_in_a_previous_skill_repo_is_not_blocked_by_its_leftover_hook() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let second = sandbox.root().join("second-repo");
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&second)
        .assert()
        .code(0);
    write(&sandbox, "skills/pdf/SKILL.md", NO_DESCRIPTION);
    sandbox.git(sandbox.skill_repo(), ["add", "--all"]);

    git(&sandbox)
        .args(["commit", "--quiet", "-m", "in the old repo"])
        .assert()
        .success()
        .stderr_eq(str![[r#"
skiman: [..]/skill-repo is not the skill repo ([..]/second-repo), so its skiman pre-commit hook checks nothing
...
"#]]);
    assert_eq!(
        sandbox.git(sandbox.skill_repo(), ["log", "--format=%s"]),
        "in the old repo"
    );
}

// ------------------------------------------------- staging what the index lacks

/// `skiman add` of upstream `acme/pdf`, one skill at its root.
fn add_pdf(sandbox: &Sandbox) -> Upstream {
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", PDF_SKILL_MD)
        .write("reference.md", "# Reference\n");
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
    up
}

/// Runs a git commit that pre-commit rejects; returns its stderr, where git
/// also shows the hook's stdout.
#[track_caller]
fn rejected(commit: Command) -> String {
    let output = commit.assert().code(1).get_output().clone();
    String::from_utf8(output.stderr).unwrap()
}

/// The printed line that stages the skill repo's paths.
#[track_caller]
fn stage_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("git -C ") && l.contains(" add -A -- "))
        .unwrap_or_else(|| panic!("no line to stage in:\n{stderr}"))
        .to_owned()
}

/// Runs `line` with `sh -c` in `dir`, as pasted into a shell there.
fn run_line(sandbox: &Sandbox, line: &str, dir: impl AsRef<Path>) {
    sandbox
        .command("sh")
        .current_dir(dir.as_ref())
        .args(["-c", line])
        .assert()
        .success();
}

/// `git commit -m`, without `-a`; asserts that pre-commit accepted it.
fn plain_commit(sandbox: &Sandbox, message: &str) {
    git(sandbox)
        .args(["commit", "--quiet", "-m", message])
        .assert()
        .success();
}

/// Runs the line `stderr` prints in `dir`, then commits with `git commit -m`.
#[track_caller]
fn stage_as_printed_and_commit(sandbox: &Sandbox, stderr: &str, dir: impl AsRef<Path>) {
    run_line(sandbox, &stage_line(stderr), dir);
    plain_commit(sandbox, "as printed");
}

fn committed_files(sandbox: &Sandbox) -> String {
    sandbox.git(
        sandbox.skill_repo(),
        ["ls-tree", "-r", "--name-only", "HEAD"],
    )
}

/// `add` after a first commit, then `git commit -am`, which leaves the new
/// folders and `skiman.lock` unstaged: rejected with the leftover records.
fn add_then_commit_am(sandbox: &Sandbox) -> String {
    init(sandbox);
    commit_all(sandbox, "layout");
    add_pdf(sandbox);
    let stderr = rejected(git(sandbox).args(["commit", "--quiet", "-am", "add pdf"]));
    assert!(stderr.contains("leftover-record"), "{stderr}");
    stderr
}

#[track_caller]
fn assert_pdf_committed(sandbox: &Sandbox) {
    let files = committed_files(sandbox);
    for path in [
        "skiman.lock",
        "skills/pdf/SKILL.md",
        "upstream/pdf/SKILL.md",
    ] {
        assert!(files.lines().any(|l| l == path), "{path} not in:\n{files}");
    }
}

#[test]
fn after_add_and_commit_am_the_printed_line_stages_the_new_skill_for_a_plain_commit() {
    let sandbox = Sandbox::new();
    let stderr = add_then_commit_am(&sandbox);

    stage_as_printed_and_commit(&sandbox, &stderr, sandbox.skill_repo());

    assert_pdf_committed(&sandbox);
}

#[test]
fn the_printed_line_does_the_same_from_a_skill_folder_and_from_outside_the_skill_repo() {
    for dir in [Some("skills/pdf"), None] {
        let sandbox = Sandbox::new();
        let stderr = add_then_commit_am(&sandbox);
        let dir = dir.map_or_else(
            || sandbox.root().to_owned(),
            |d| sandbox.skill_repo().join(d),
        );

        stage_as_printed_and_commit(&sandbox, &stderr, &dir);

        assert_pdf_committed(&sandbox);
    }
}

#[test]
fn after_the_first_add_with_only_the_skill_folders_staged_the_printed_line_stages_the_records() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    commit_all(&sandbox, "layout");
    add_pdf(&sandbox);
    sandbox.git(sandbox.skill_repo(), ["add", "skills/pdf", "upstream/pdf"]);

    let stderr = rejected(git(&sandbox).args(["commit", "--quiet", "-m", "add pdf"]));
    stage_as_printed_and_commit(&sandbox, &stderr, sandbox.skill_repo());

    assert_pdf_committed(&sandbox);
}

#[test]
fn after_an_update_adds_a_file_and_commit_am_the_printed_line_stages_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = add_pdf(&sandbox);
    commit_all(&sandbox, "add pdf");
    up.write("forms.md", "# Forms\n");
    up.commit("v2");
    sandbox.skiman().args(["update", "pdf"]).assert().code(0);

    let stderr = rejected(git(&sandbox).args(["commit", "--quiet", "-am", "update pdf"]));
    assert!(stderr.contains("snapshot-hash"), "{stderr}");
    stage_as_printed_and_commit(&sandbox, &stderr, sandbox.skill_repo());

    let files = committed_files(&sandbox);
    for path in ["skills/pdf/forms.md", "upstream/pdf/forms.md"] {
        assert!(files.lines().any(|l| l == path), "{path} not in:\n{files}");
    }
}

#[test]
fn a_secret_staged_in_the_shared_body_but_fixed_on_disk_is_staged_away_with_the_entries_last() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    commit_all(&sandbox, "layout");
    let repo = sandbox.skill_repo();
    let shared = std::fs::read_to_string(repo.join("instructions/shared.md")).unwrap();
    let key = ["AK", "IA", "Q2W3E4R5T6Y7U8I9"].concat();
    write(
        &sandbox,
        "instructions/shared.md",
        &format!("{shared}\nkey {key}\n"),
    );
    sandbox.git(&repo, ["add", "--all"]);
    write(&sandbox, "instructions/shared.md", &shared);
    write(&sandbox, "skills/docx/SKILL.md", &skill_md("docx"));

    let stderr = rejected(git(&sandbox).args(["commit", "--quiet", "-m", "notes"]));
    assert!(!stderr.contains(&key), "{stderr}");
    let line = stderr.find(&stage_line(&stderr)).unwrap();
    let entries = stderr
        .find("If a match is not a real secret, add its entry to skiman.toml:")
        .unwrap_or_else(|| panic!("no entries in:\n{stderr}"));
    assert!(line < entries, "the entries must come last:\n{stderr}");
    stage_as_printed_and_commit(&sandbox, &stderr, &repo);

    assert_eq!(
        sandbox.git(&repo, ["show", "HEAD:instructions/shared.md"]),
        shared.trim_end_matches('\n')
    );
    assert!(committed_files(&sandbox).contains("skills/docx/SKILL.md"));
}

#[test]
fn a_symlink_staged_in_a_new_skill_and_deleted_in_the_working_tree_is_staged_away() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    commit_all(&sandbox, "layout");
    let repo = sandbox.skill_repo();
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    std::os::unix::fs::symlink("SKILL.md", repo.join("skills/pdf/link.md")).unwrap();
    sandbox.git(&repo, ["add", "--all"]);
    std::fs::remove_file(repo.join("skills/pdf/link.md")).unwrap();

    let stderr = rejected(git(&sandbox).args(["commit", "--quiet", "-m", "add pdf"]));
    assert!(stderr.contains("skills/pdf/link.md: symlink: "), "{stderr}");
    stage_as_printed_and_commit(&sandbox, &stderr, &repo);

    let files = committed_files(&sandbox);
    assert!(files.lines().any(|l| l == "skills/pdf/SKILL.md"), "{files}");
    assert!(!files.contains("link.md"), "{files}");
}

#[test]
fn after_the_last_project_is_ejected_the_printed_line_stages_the_mirror_deletion_too() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let project = sandbox.home().join("work");
    std::fs::create_dir_all(project.join("a")).unwrap();
    std::fs::write(project.join("a/AGENTS.md"), "# Agents\n").unwrap();
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(&project)
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    commit_all(&sandbox, "project");
    sandbox
        .skiman()
        .args(["project", "eject", "--yes"])
        .arg(&project)
        .assert()
        .code(0);
    assert!(!sandbox.skill_repo().join("projects").exists());
    add_pdf(&sandbox);

    // `git commit -a` checks a temporary index that no longer has
    // `projects/`; the line must still name it, or the deletion stays unstaged.
    let stderr = rejected(git(&sandbox).args(["commit", "--quiet", "-am", "add pdf"]));
    assert!(stage_line(&stderr).ends_with(" projects"), "{stderr}");
    stage_as_printed_and_commit(&sandbox, &stderr, sandbox.skill_repo());

    assert_pdf_committed(&sandbox);
    assert!(!committed_files(&sandbox).contains("projects/"));
}

#[test]
fn no_line_is_printed_when_the_working_tree_breaks_the_rules_too() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    add_pdf(&sandbox);
    commit_all(&sandbox, "add pdf");
    std::fs::remove_dir_all(sandbox.skill_repo().join("skills/pdf")).unwrap();

    let stderr = rejected(git(&sandbox).args(["commit", "--quiet", "-am", "drop pdf"]));

    assert!(stderr.contains("leftover-record"), "{stderr}");
    assert!(!stderr.contains("git -C"), "{stderr}");
}

#[test]
fn the_printed_line_cannot_stage_what_a_clean_filter_breaks_so_the_commit_is_rejected_again() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    sandbox.git(
        &repo,
        ["config", "filter.strip.clean", "sed /^description:/d"],
    );
    write(
        &sandbox,
        ".gitattributes",
        "skills/*/SKILL.md filter=strip\n",
    );
    commit_all(&sandbox, "layout");
    write(&sandbox, "skills/pdf/SKILL.md", &skill_md("pdf"));
    sandbox.git(&repo, ["add", "--all"]);

    let first = rejected(git(&sandbox).args(["commit", "--quiet", "-m", "add pdf"]));
    run_line(&sandbox, &stage_line(&first), &repo);

    git(&sandbox)
        .args(["commit", "--quiet", "-m", "add pdf"])
        .assert()
        .code(1)
        .stderr_eq(first.as_str());
}
