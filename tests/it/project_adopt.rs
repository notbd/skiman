//! `skiman project adopt`: discovery under a project root, the gates that
//! keep a candidate out of the mirror, the copy into `projects/<id>/`, and
//! the exact-path rule (a folder never replaces; a named item may).
//!
//! Every test registers `~/work` as project root `work`, so a project item
//! at `<root>/<rel>` has its mirror at `<skill repo>/projects/work/<rel>`.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::support::{
    Sandbox, aws_key, backups, exists, hex, init, only_backup, read, skill_md, tree, tree_hash,
    write,
};

// ---------------------------------------------------------------- helpers

/// The project root every test uses.
fn root(sandbox: &Sandbox) -> PathBuf {
    sandbox.home().join("work")
}

/// `init`, then registers `~/work` with `project add`; `extra` lines
/// (`exclude = …`) are appended to its table.
fn setup(extra: &str) -> Sandbox {
    let sandbox = Sandbox::new();
    init(&sandbox);
    std::fs::create_dir_all(root(&sandbox)).unwrap();
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(root(&sandbox))
        .assert()
        .code(0);
    if !extra.is_empty() {
        let manifest = sandbox.skill_repo().join("skiman.toml");
        let mut text = read(&manifest);
        assert!(text.trim_end().ends_with("path = \"~/work\""), "{text}");
        text.push_str(extra);
        text.push('\n');
        std::fs::write(manifest, text).unwrap();
    }
    sandbox
}

/// A project skill folder holding a valid `SKILL.md`.
fn skill(dir: impl AsRef<Path>) {
    let dir = dir.as_ref();
    let name = dir.file_name().unwrap().to_str().unwrap();
    write(dir.join("SKILL.md"), skill_md(name));
}

/// The mirror of the root-relative `rel`.
fn mirror(sandbox: &Sandbox, rel: &str) -> PathBuf {
    sandbox.skill_repo().join("projects/work").join(rel)
}

fn state_dir(sandbox: &Sandbox) -> PathBuf {
    sandbox.state_home().join("skiman")
}

fn synced_file(sandbox: &Sandbox) -> PathBuf {
    state_dir(sandbox).join("projects-synced.toml")
}

/// The skill repo (without `.git`) and skiman's machine state, to prove a
/// command changed nothing.
fn world(sandbox: &Sandbox) -> (BTreeMap<PathBuf, String>, BTreeMap<PathBuf, String>) {
    let mut repo = tree(sandbox.skill_repo());
    repo.retain(|path, _| path.components().next().unwrap().as_os_str() != ".git");
    (repo, tree(state_dir(sandbox)))
}

/// `skiman project adopt` with `args`.
fn adopt<I, S>(sandbox: &Sandbox, args: I) -> snapbox::cmd::Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    sandbox.skiman().args(["project", "adopt"]).args(args)
}

/// stdout and stderr of a finished command that exited with `code`.
#[track_caller]
fn run(command: snapbox::cmd::Command, code: i32) -> (String, String) {
    let output = command.assert().code(code).get_output().clone();
    (
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// The root-relative paths a `--dry-run` plans to adopt.
#[track_caller]
fn planned(sandbox: &Sandbox) -> BTreeSet<String> {
    let (stdout, _) = run(adopt(sandbox, ["--dry-run"]), 0);
    let prefix = format!("adopt {}/", root(sandbox).display());
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .map(str::to_owned)
        .collect()
}

fn set(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

/// `sha256:<hex>` of `bytes`, the form skiman records for a file item.
fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex(&Sha256::digest(bytes)))
}

// ---------------------------------------------------------------- discovery

#[test]
fn discovery_finds_every_instruction_file_pattern_and_every_skill_dir() {
    let sandbox = setup("");
    let root = root(&sandbox);
    for rel in [
        "AGENTS.md",
        ".claude/rules/top.md",
        "a/CLAUDE.md",
        "a/CLAUDE.local.md",
        "a/.claude/CLAUDE.md",
        "a/.claude/rules/style.md",
        "a/.claude/rules/sub/deep/naming.md",
        "b/AGENTS.override.md",
        "b/GEMINI.md",
        // Not project items.
        "a/README.md",
        "a/.claude/rules/notes.txt",
        "a/.claude/settings.json",
        "b/docs/guide.md",
    ] {
        write(root.join(rel), format!("# {rel}\n"));
    }
    for rel in [
        "c/.claude/skills/one",
        "c/.agents/skills/two",
        "c/.agent/skills/three",
        // No tool loads skills from these.
        "c/.gemini/skills/five",
        "other/skills/six",
    ] {
        skill(root.join(rel));
    }
    // A folder in a skill dir without `SKILL.md` is no skill.
    write(root.join("c/.claude/skills/empty/README.md"), "notes\n");

    assert_eq!(
        planned(&sandbox),
        set(&[
            ".claude/rules/top.md",
            "AGENTS.md",
            "a/.claude/CLAUDE.md",
            "a/.claude/rules/style.md",
            "a/.claude/rules/sub/deep/naming.md",
            "a/CLAUDE.local.md",
            "a/CLAUDE.md",
            "b/AGENTS.override.md",
            "b/GEMINI.md",
            "c/.agent/skills/three",
            "c/.agents/skills/two",
            "c/.claude/skills/one",
        ])
    );
}

#[test]
fn discovery_does_not_enter_a_skill_folder() {
    let sandbox = setup("");
    let skill_dir = root(&sandbox).join("p/.claude/skills/s");
    skill(&skill_dir);
    write(skill_dir.join("AGENTS.md"), "part of the skill\n");
    write(skill_dir.join("ref/CLAUDE.md"), "part of the skill\n");
    skill(skill_dir.join(".claude/skills/inner"));

    assert_eq!(planned(&sandbox), set(&["p/.claude/skills/s"]));

    adopt(&sandbox, ["--yes"]).assert().code(0);
    assert_eq!(
        tree(mirror(&sandbox, "p/.claude/skills/s")),
        tree(&skill_dir)
    );
}

#[test]
fn discovery_does_not_follow_directory_symlinks_and_ignores_symlinked_items() {
    let sandbox = setup("");
    let root = root(&sandbox);
    let outside = sandbox.home().join("outside");
    write(outside.join("AGENTS.md"), "outside\n");
    skill(outside.join(".claude/skills/o"));
    skill(outside.join("global-skill"));
    write(root.join("p/AGENTS.md"), "# p\n");
    std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
    std::os::unix::fs::symlink("AGENTS.md", root.join("p/CLAUDE.md")).unwrap();
    std::fs::create_dir_all(root.join("p/.claude/rules")).unwrap();
    std::os::unix::fs::symlink("../../AGENTS.md", root.join("p/.claude/rules/r.md")).unwrap();
    std::fs::create_dir_all(root.join("p/.claude/skills")).unwrap();
    std::os::unix::fs::symlink(
        outside.join("global-skill"),
        root.join("p/.claude/skills/g"),
    )
    .unwrap();

    let (stdout, stderr) = run(adopt(&sandbox, ["--dry-run"]), 0);
    assert_eq!(
        stdout,
        format!("adopt {}\n", root.join("p/AGENTS.md").display())
    );
    assert_eq!(stderr, "", "an alias link is not reported");
}

#[test]
fn discovery_skips_git_dirs_node_modules_and_linked_worktrees() {
    let sandbox = setup("");
    let root = root(&sandbox);
    // A project repo: its `.git` is never entered, its untracked files are candidates.
    std::fs::create_dir_all(root.join("app")).unwrap();
    sandbox.git(root.join("app"), ["init", "--quiet"]);
    write(root.join("app/AGENTS.md"), "# app\n");
    write(root.join("app/.git/AGENTS.md"), "inside .git\n");
    skill(root.join("app/.git/.claude/skills/x"));
    write(
        root.join("web/node_modules/pkg/AGENTS.md"),
        "a dependency\n",
    );
    skill(root.join("web/node_modules/pkg/.claude/skills/y"));
    // A linked worktree of a repo outside the root.
    let main = sandbox.home().join("main-repo");
    write(main.join("README.md"), "main\n");
    sandbox.git(&main, ["init", "--quiet"]);
    sandbox.git(&main, ["add", "README.md"]);
    sandbox.git(&main, ["commit", "--quiet", "-m", "init"]);
    sandbox.git(
        &main,
        [
            "worktree",
            "add",
            "--quiet",
            root.join("wt").to_str().unwrap(),
        ],
    );
    write(root.join("wt/AGENTS.md"), "untracked in a worktree\n");

    assert_eq!(planned(&sandbox), set(&["app/AGENTS.md"]));
}

#[test]
fn exclude_by_name_matches_at_any_depth_and_by_path_only_under_that_path() {
    let sandbox = setup("exclude = [\"_archive\", \"a/old\"]");
    let root = root(&sandbox);
    for rel in [
        "_archive/AGENTS.md",
        "x/_archive/y/AGENTS.md",
        "a/old/AGENTS.md",
        "a/old/sub/CLAUDE.md",
        "a/AGENTS.md",
        "a/older/AGENTS.md",
        "a/x/old/AGENTS.md",
    ] {
        write(root.join(rel), format!("# {rel}\n"));
    }
    skill(root.join("a/old/.claude/skills/s"));
    skill(root.join("_archive/.claude/skills/t"));

    assert_eq!(
        planned(&sandbox),
        set(&["a/AGENTS.md", "a/older/AGENTS.md", "a/x/old/AGENTS.md"])
    );
}

#[test]
fn an_exact_path_wins_over_exclude() {
    let sandbox = setup("exclude = [\"_archive\"]");
    let file = root(&sandbox).join("_archive/AGENTS.md");
    write(&file, "# archived\n");

    adopt(&sandbox, [&file]).arg("--yes").assert().code(0);
    assert_eq!(read(mirror(&sandbox, "_archive/AGENTS.md")), "# archived\n");
}

// ---------------------------------------------------------------- gates

#[test]
fn items_a_project_repo_tracks_are_skipped_and_its_untracked_items_are_adopted() {
    let sandbox = setup("");
    let repo = root(&sandbox).join("app");
    std::fs::create_dir_all(&repo).unwrap();
    sandbox.git(&repo, ["init", "--quiet"]);
    write(repo.join("AGENTS.md"), "# tracked\n");
    let skill_dir = repo.join(".claude/skills/s");
    skill(&skill_dir);
    write(skill_dir.join("scripts/run.sh"), "echo run\n");
    write(repo.join("sub/CLAUDE.md"), "# untracked\n");
    // Only one file of the skill is tracked: the whole skill stays with the repo.
    sandbox.git(
        &repo,
        ["add", "AGENTS.md", ".claude/skills/s/scripts/run.sh"],
    );

    let (stdout, stderr) = run(adopt(&sandbox, ["--yes"]), 0);
    assert!(stdout.contains(&format!("adopted {}", repo.join("sub/CLAUDE.md").display())));
    assert!(
        stderr.contains(&format!(
            "skip 2 item(s) tracked by the git repo {}",
            repo.display()
        )),
        "{stderr}"
    );
    assert!(stderr.contains(&format!("  {}\n", repo.join("AGENTS.md").display())));
    assert!(stderr.contains(&format!("  {}\n", skill_dir.display())));
    assert_eq!(read(mirror(&sandbox, "app/sub/CLAUDE.md")), "# untracked\n");
    assert!(!exists(mirror(&sandbox, "app/AGENTS.md")));
    assert!(!exists(mirror(&sandbox, "app/.claude/skills/s")));

    // Named exactly, a skipped item fails the command.
    adopt(&sandbox, [repo.join("AGENTS.md")])
        .arg("--yes")
        .assert()
        .code(1);
    assert!(!exists(mirror(&sandbox, "app/AGENTS.md")));
}

#[test]
fn a_secret_blocks_the_item_with_the_allow_hint_and_the_pasted_entry_lets_it_in() {
    let sandbox = setup("");
    let key = aws_key();
    let file = root(&sandbox).join("p/AGENTS.md");
    write(&file, format!("# p\nAWS_ACCESS_KEY_ID={key}\n"));
    write(root(&sandbox).join("q/AGENTS.md"), "# q\n");

    let (stdout, stderr) = run(adopt(&sandbox, ["--yes"]), 0);
    assert!(
        stdout.contains("adopted") && stdout.contains("q/AGENTS.md"),
        "{stdout}"
    );
    assert!(
        stderr.contains(&format!(
            "skip {}: the project's version breaks the rules:",
            file.display()
        )),
        "{stderr}"
    );
    assert!(stderr.contains("aws-access-key"), "{stderr}");
    assert!(stderr.contains("[[secrets.allow]]"), "{stderr}");
    assert!(
        !stdout.contains(&key) && !stderr.contains(&key),
        "the secret leaked"
    );
    assert!(!exists(mirror(&sandbox, "p/AGENTS.md")));

    // Named exactly: exit 1.
    let (_, stderr) = run(adopt(&sandbox, [&file]).arg("--yes"), 1);
    assert!(!exists(mirror(&sandbox, "p/AGENTS.md")));

    // The hint is paste-ready.
    let entry: String = stderr
        .lines()
        .skip_while(|l| *l != "[[secrets.allow]]")
        .take_while(|l| !l.is_empty())
        .map(|l| format!("{l}\n"))
        .collect();
    assert!(
        entry.contains("path = \"projects/work/p/AGENTS.md\""),
        "{entry}"
    );
    let manifest = sandbox.skill_repo().join("skiman.toml");
    let text = format!("{}\n{entry}", read(&manifest));
    std::fs::write(&manifest, text).unwrap();
    adopt(&sandbox, [&file]).arg("--yes").assert().code(0);
    assert_eq!(read(mirror(&sandbox, "p/AGENTS.md")), read(&file));
}

#[test]
fn a_skill_with_a_symlink_inside_is_skipped() {
    let sandbox = setup("");
    let skill_dir = root(&sandbox).join("p/.claude/skills/s");
    skill(&skill_dir);
    std::os::unix::fs::symlink("SKILL.md", skill_dir.join("alias.md")).unwrap();

    let (_, stderr) = run(adopt(&sandbox, ["--yes"]), 0);
    assert!(
        stderr.contains(&format!("skip {}:", skill_dir.display())),
        "{stderr}"
    );
    assert!(!exists(mirror(&sandbox, "p/.claude/skills/s")));

    adopt(&sandbox, [&skill_dir]).arg("--yes").assert().code(1);
    assert!(!exists(mirror(&sandbox, "p/.claude/skills/s")));
}

#[test]
fn a_skill_folder_without_skill_md_is_refused_when_named_exactly() {
    let sandbox = setup("");
    let skill_dir = root(&sandbox).join("p/.claude/skills/s");
    write(skill_dir.join("README.md"), "no SKILL.md here\n");

    let (_, stderr) = run(adopt(&sandbox, [&skill_dir]).arg("--yes"), 1);
    assert!(stderr.contains("no SKILL.md"), "{stderr}");
    assert!(!exists(mirror(&sandbox, "p/.claude/skills/s")));
}

#[test]
fn git_conflict_markers_block_a_file_and_a_skill() {
    let sandbox = setup("");
    let conflicted = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n";
    let file = root(&sandbox).join("p/AGENTS.md");
    write(&file, conflicted);
    let skill_dir = root(&sandbox).join("p/.claude/skills/s");
    skill(&skill_dir);
    write(skill_dir.join("reference.md"), conflicted);

    let (stdout, stderr) = run(adopt(&sandbox, ["--yes"]), 0);
    assert_eq!(stdout, "nothing to adopt\n");
    for path in [&file, &skill_dir] {
        assert!(
            stderr.contains(&format!(
                "skip {}: the project's version breaks the rules:",
                path.display()
            )),
            "{stderr}"
        );
    }
    assert_eq!(stderr.matches("conflict marker").count(), 4, "{stderr}");
    assert!(!exists(sandbox.skill_repo().join("projects")));
}

#[test]
fn a_skill_without_frontmatter_name_in_a_folder_of_any_name_is_adopted() {
    let sandbox = setup("");
    let skill_dir = root(&sandbox).join("p/.claude/skills/My_Skill");
    write(
        skill_dir.join("SKILL.md"),
        "# Just instructions, no frontmatter\n",
    );

    adopt(&sandbox, ["--yes"]).assert().code(0);
    assert_eq!(
        tree(mirror(&sandbox, "p/.claude/skills/My_Skill")),
        tree(&skill_dir)
    );
}

// ---------------------------------------------------------------- adopt

/// A root with one instruction file, one rules file and one skill.
fn three_items(sandbox: &Sandbox) {
    let root = root(sandbox);
    // Bytes that a text round trip would change.
    write(
        root.join("AGENTS.md"),
        b"# Root\r\n\xe4\xb8\xad\xe6\x96\x87 \xff no newline",
    );
    write(root.join("p/.claude/rules/sub/style.md"), "Use tabs.\n");
    let skill_dir = root.join("p/.claude/skills/s");
    skill(&skill_dir);
    write(skill_dir.join("data/table.csv"), "a,b\n1,2\n");
    let script = skill_dir.join("scripts/run.sh");
    write(&script, "#!/bin/sh\necho run\n");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn adopt_copies_byte_identical_mirrors_and_leaves_the_project_untouched() {
    let sandbox = setup("");
    three_items(&sandbox);
    let project_before = tree(root(&sandbox));

    adopt(&sandbox, ["--yes"]).assert().code(0);

    assert_eq!(
        tree(root(&sandbox)),
        project_before,
        "the project is untouched"
    );
    let mut mirrored = tree(sandbox.skill_repo().join("projects/work"));
    let mut project = project_before;
    // Folders that only lead to items are on both sides; compare the rest.
    mirrored.retain(|_, v| v != "dir");
    project.retain(|_, v| v != "dir");
    assert_eq!(
        mirrored, project,
        "the mirror holds exactly the items, mode included"
    );
    assert_eq!(
        std::fs::read(mirror(&sandbox, "AGENTS.md")).unwrap(),
        std::fs::read(root(&sandbox).join("AGENTS.md")).unwrap()
    );
}

#[test]
fn adopt_records_the_synced_hash_of_every_item_in_machine_state() {
    let sandbox = setup("");
    three_items(&sandbox);
    let root = root(&sandbox);

    adopt(&sandbox, ["--yes"]).assert().code(0);

    let text = read(synced_file(&sandbox));
    assert!(
        text.contains(&format!("[work]\nroot = \"{}\"", root.display())),
        "{text}"
    );
    let file_hash = sha256(&std::fs::read(root.join("AGENTS.md")).unwrap());
    let rules_hash = sha256(&std::fs::read(root.join("p/.claude/rules/sub/style.md")).unwrap());
    let skill_hash = tree_hash(&root.join("p/.claude/skills/s"));
    for line in [
        format!("\"AGENTS.md\" = \"file {file_hash}\""),
        format!("\"p/.claude/rules/sub/style.md\" = \"file {rules_hash}\""),
        format!("\"p/.claude/skills/s\" = \"skill {skill_hash}\""),
    ] {
        assert!(text.contains(&line), "{line} missing from:\n{text}");
    }
}

#[test]
fn dry_run_prints_the_plan_and_writes_nothing() {
    let sandbox = setup("");
    three_items(&sandbox);
    let before = world(&sandbox);
    let project_before = tree(root(&sandbox));

    let (stdout, stderr) = run(adopt(&sandbox, ["--dry-run"]), 0);
    let root = root(&sandbox);
    assert_eq!(
        stdout,
        format!(
            "adopt {}\nadopt {}\nadopt {}\n",
            root.join("AGENTS.md").display(),
            root.join("p/.claude/rules/sub/style.md").display(),
            root.join("p/.claude/skills/s").display()
        )
    );
    assert_eq!(stderr, "");
    assert_eq!(world(&sandbox), before);
    assert_eq!(tree(&root), project_before);
    assert!(!exists(synced_file(&sandbox)));
}

#[test]
fn a_second_adopt_is_a_no_op() {
    let sandbox = setup("");
    three_items(&sandbox);
    adopt(&sandbox, ["--yes"]).assert().code(0);
    let before = world(&sandbox);

    let (stdout, stderr) = run(adopt(&sandbox, ["--yes"]), 0);
    assert_eq!(stdout, "nothing to adopt\n");
    assert_eq!(stderr, "");
    assert_eq!(world(&sandbox), before);
}

#[test]
fn without_a_terminal_adopt_needs_yes_and_writes_nothing() {
    let sandbox = setup("");
    three_items(&sandbox);
    let before = world(&sandbox);

    let (stdout, stderr) = run(adopt(&sandbox, [] as [&str; 0]), 2);
    assert_eq!(stdout, "");
    assert!(stderr.contains("--yes"), "{stderr}");
    assert_eq!(world(&sandbox), before);
}

#[test]
fn adopt_refuses_while_the_skill_repo_is_mid_merge_or_detached() {
    let sandbox = setup("");
    three_items(&sandbox);
    let repo = sandbox.skill_repo();
    sandbox.git(
        &repo,
        [
            "commit",
            "--quiet",
            "--allow-empty",
            "--no-verify",
            "-m",
            "init",
        ],
    );
    let head = sandbox.git(&repo, ["rev-parse", "HEAD"]);

    write(repo.join(".git/MERGE_HEAD"), format!("{head}\n"));
    let before = world(&sandbox);
    let (_, stderr) = run(adopt(&sandbox, ["--yes"]), 4);
    assert!(stderr.contains("is in the middle of a merge"), "{stderr}");
    assert_eq!(world(&sandbox), before);
    std::fs::remove_file(repo.join(".git/MERGE_HEAD")).unwrap();

    sandbox.git(&repo, ["checkout", "--quiet", "--detach"]);
    let before = world(&sandbox);
    let (_, stderr) = run(adopt(&sandbox, ["--yes"]), 4);
    assert!(stderr.contains("detached HEAD"), "{stderr}");
    assert_eq!(world(&sandbox), before);
}

// ---------------------------------------------------------------- exact path vs folder

/// Adopts `<root>/<rel>` files holding `contents`, one per `(rel, contents)`.
fn adopted_files(sandbox: &Sandbox, files: &[(&str, &str)]) {
    for (rel, contents) in files {
        write(root(sandbox).join(rel), contents);
    }
    adopt(sandbox, ["--yes"]).assert().code(0);
}

#[test]
fn a_folder_adopt_never_replaces_a_mirror_or_undoes_a_mirror_deletion() {
    let sandbox = setup("");
    adopted_files(
        &sandbox,
        &[
            ("a/AGENTS.md", "a\n"),
            ("b/AGENTS.md", "b\n"),
            ("c/AGENTS.md", "c\n"),
        ],
    );
    let root = root(&sandbox);
    // a: conflict; b: mirror changed; c: deleted in the mirror.
    write(root.join("a/AGENTS.md"), "a, project edit\n");
    write(mirror(&sandbox, "a/AGENTS.md"), "a, mirror edit\n");
    write(mirror(&sandbox, "b/AGENTS.md"), "b, mirror edit\n");
    std::fs::remove_file(mirror(&sandbox, "c/AGENTS.md")).unwrap();
    // Something new, so the command writes and ends with its sync.
    write(root.join("d/AGENTS.md"), "d\n");
    let project_before = tree(&root);

    let (stdout, stderr) = run(adopt(&sandbox, [&root]).arg("--yes"), 3);
    assert!(stdout.contains(&format!("adopted {}", root.join("d/AGENTS.md").display())));
    assert!(!stdout.contains("keep the project's version"), "{stdout}");
    assert!(
        stderr.contains("both changed"),
        "the sync reports the conflict: {stderr}"
    );
    assert_eq!(read(mirror(&sandbox, "a/AGENTS.md")), "a, mirror edit\n");
    assert_eq!(read(mirror(&sandbox, "b/AGENTS.md")), "b, mirror edit\n");
    assert!(!exists(mirror(&sandbox, "c/AGENTS.md")));
    assert_eq!(read(mirror(&sandbox, "d/AGENTS.md")), "d\n");
    assert_eq!(tree(&root), project_before);
    assert!(!exists(backups(&sandbox)), "nothing was replaced");
}

#[test]
fn a_folder_adopt_with_nothing_new_still_reports_a_conflict() {
    let sandbox = setup("");
    adopted_files(&sandbox, &[("AGENTS.md", "v1\n")]);
    write(root(&sandbox).join("AGENTS.md"), "project edit\n");
    write(mirror(&sandbox, "AGENTS.md"), "mirror edit\n");

    // `adopt` ends with `sync`, which reports row 15 with exit 3, as it
    // does when the same command also adopts a new item.
    let (_, stderr) = run(adopt(&sandbox, ["--yes"]), 3);
    assert!(stderr.contains("both changed"), "{stderr}");
}

#[test]
fn exact_adopt_of_a_conflict_keeps_the_project_version_and_backs_up_the_mirror() {
    let sandbox = setup("");
    adopted_files(&sandbox, &[("p/AGENTS.md", "v1\n")]);
    let file = root(&sandbox).join("p/AGENTS.md");
    write(&file, "project edit\n");
    write(mirror(&sandbox, "p/AGENTS.md"), "mirror edit\n");

    let (stdout, _) = run(adopt(&sandbox, [&file]).arg("--yes"), 0);
    assert!(
        stdout.contains(&format!("keep the project's version of {}", file.display())),
        "{stdout}"
    );
    assert_eq!(read(&file), "project edit\n");
    assert_eq!(read(mirror(&sandbox, "p/AGENTS.md")), "project edit\n");
    let backup = only_backup(&sandbox);
    assert!(stdout.contains(&format!("replaced versions are in {}", backup.display())));
    assert_eq!(
        read(backup.join("repo/projects/work/p/AGENTS.md")),
        "mirror edit\n"
    );
    assert!(
        read(synced_file(&sandbox)).contains(&sha256(b"project edit\n")),
        "the synced hash is the project's version"
    );
}

/// A mirror staging folder that cannot be made fails its item only: adopt
/// writes the items before it, saves the synced record and says where the
/// replaced versions are.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_staging_folder_that_cannot_be_made_fails_its_item_only() {
    let sandbox = setup("");
    let files = [("a/AGENTS.md", "v1\n"), ("b/AGENTS.md", "v1\n")];
    adopted_files(&sandbox, &files);
    for (rel, _) in files {
        write(root(&sandbox).join(rel), "project edit\n");
        write(mirror(&sandbox, rel), "mirror edit\n");
    }
    let swap = sandbox.skill_repo().join(".skiman/swap");
    // Once the first item is staged, no other staging folder can be made.
    let chmod = format!("chmod 555 '{}'", swap.display());
    let items = files.map(|(rel, _)| root(&sandbox).join(rel));
    let out = adopt(&sandbox, &items)
        .arg("--yes")
        .env(
            "SKIMAN_TEST_HOOK",
            "project:write:projects/work/a/AGENTS.md",
        )
        .env("SKIMAN_TEST_HOOK_RUN", chmod)
        .output()
        .unwrap();
    std::fs::set_permissions(&swap, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (said, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(out.status.code(), Some(4), "{said}{stderr}");
    assert!(stderr.contains("cannot create a folder in"), "{stderr}");
    assert_eq!(read(mirror(&sandbox, "a/AGENTS.md")), "project edit\n");
    assert_eq!(read(mirror(&sandbox, "b/AGENTS.md")), "mirror edit\n");
    assert!(said.contains("replaced versions are in "), "{said}");
    assert!(
        read(synced_file(&sandbox)).contains(&sha256(b"project edit\n")),
        "the synced record of a/AGENTS.md is saved"
    );
}

#[test]
fn exact_adopt_of_a_changed_skill_mirror_keeps_the_project_version_and_backs_up_the_mirror() {
    let sandbox = setup("");
    let skill_dir = root(&sandbox).join("p/.claude/skills/s");
    skill(&skill_dir);
    write(skill_dir.join("reference.md"), "v1\n");
    adopt(&sandbox, ["--yes"]).assert().code(0);
    let skill_mirror = mirror(&sandbox, "p/.claude/skills/s");
    write(skill_mirror.join("reference.md"), "mirror edit\n");
    write(skill_mirror.join("extra.md"), "only in the mirror\n");
    let edited_mirror = tree(&skill_mirror);

    adopt(&sandbox, [&skill_dir]).arg("--yes").assert().code(0);
    assert_eq!(tree(&skill_mirror), tree(&skill_dir));
    assert_eq!(
        tree(only_backup(&sandbox).join("repo/projects/work/p/.claude/skills/s")),
        edited_mirror
    );
}

#[test]
fn exact_adopt_of_an_item_deleted_in_the_mirror_takes_it_back_in() {
    let sandbox = setup("");
    adopted_files(&sandbox, &[("p/AGENTS.md", "v1\n")]);
    let file = root(&sandbox).join("p/AGENTS.md");
    std::fs::remove_file(mirror(&sandbox, "p/AGENTS.md")).unwrap();

    adopt(&sandbox, [&file]).arg("--yes").assert().code(0);
    assert_eq!(read(mirror(&sandbox, "p/AGENTS.md")), "v1\n");
    assert!(!exists(backups(&sandbox)), "nothing was replaced");
    assert!(read(synced_file(&sandbox)).contains(&sha256(b"v1\n")));
}

#[test]
fn an_exact_path_inside_a_skill_folder_is_not_an_item_of_its_own() {
    let sandbox = setup("");
    let skill_dir = root(&sandbox).join("p/.claude/skills/s");
    skill(&skill_dir);
    write(skill_dir.join("AGENTS.md"), "part of the skill\n");

    // The file is part of skill `s`, not a project item: adopting it alone
    // would leave `projects/work/p/.claude/skills/s` a skill mirror without
    // `SKILL.md`.
    adopt(&sandbox, [skill_dir.join("AGENTS.md")])
        .arg("--yes")
        .assert()
        .code(1);
    assert!(!exists(mirror(&sandbox, "p/.claude/skills/s")));
    adopt(&sandbox, [&skill_dir]).arg("--yes").assert().code(0);
    assert_eq!(
        tree(mirror(&sandbox, "p/.claude/skills/s")),
        tree(&skill_dir)
    );
}

#[test]
fn a_project_skill_that_is_a_git_clone_of_its_own_is_left_to_that_repo() {
    let sandbox = setup("");
    let skill_dir = root(&sandbox).join("p/.claude/skills/cloned");
    write(skill_dir.join("SKILL.md"), skill_md("cloned"));
    sandbox.git(&skill_dir, ["init", "-q"]);
    sandbox.git(&skill_dir, ["add", "SKILL.md"]);

    let assert = sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("tracked by the git repo"), "{stderr}");
    assert!(!exists(mirror(&sandbox, "p/.claude/skills/cloned")));
}

#[test]
fn an_exact_path_wins_over_a_folder_named_before_it() {
    let sandbox = setup("");
    let item = root(&sandbox).join("p/AGENTS.md");
    write(&item, "v1\n");
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    write(&item, "project\n");
    write(mirror(&sandbox, "p/AGENTS.md"), "mirror\n");

    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .arg(root(&sandbox))
        .arg(&item)
        .assert()
        .code(0);
    assert_eq!(read(mirror(&sandbox, "p/AGENTS.md")), "project\n");
}

#[test]
fn a_root_that_holds_the_skill_repo_skips_it() {
    let sandbox = Sandbox::new();
    let holder = sandbox.home().join("dev");
    let skill_repo = holder.join("skills");
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&skill_repo)
        .assert()
        .code(0);
    write(holder.join("proj/AGENTS.md"), "p\n");
    sandbox
        .skiman()
        .args(["project", "add", "--id", "dev"])
        .arg(&holder)
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    let mirrored = skill_repo.join("projects/dev");
    assert_eq!(read(mirrored.join("proj/AGENTS.md")), "p\n");
    assert!(
        !mirrored.join("skills").exists(),
        "the canon is never discovered"
    );
}

#[test]
fn an_exact_path_inside_the_skill_repo_is_refused_and_nothing_is_mirrored_twice() {
    let sandbox = Sandbox::new();
    let holder = sandbox.home().join("dev");
    let skill_repo = holder.join("skills");
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&skill_repo)
        .assert()
        .code(0);
    write(holder.join("proj/AGENTS.md"), "p\n");
    sandbox
        .skiman()
        .args(["project", "add", "--id", "dev"])
        .arg(&holder)
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    let own_mirror = skill_repo.join("projects/dev/proj/AGENTS.md");
    let world = || {
        let mut repo = tree(&skill_repo);
        repo.retain(|path, _| !path.starts_with(".git"));
        (repo, tree(state_dir(&sandbox)))
    };
    let before = world();

    for verb in ["adopt", "restore"] {
        let (stdout, stderr) = run(
            sandbox
                .skiman()
                .args(["project", verb, "--yes"])
                .arg(&own_mirror),
            1,
        );

        assert_eq!(stdout, "");
        assert_eq!(
            stderr,
            format!(
                "error: {} is inside {}, which project commands never read or write; name the item's path in its project\n",
                own_mirror.display(),
                skill_repo.display()
            )
        );
        assert_eq!(world(), before);
    }
    assert!(!exists(skill_repo.join("projects/dev/skills")));
}

#[test]
fn the_warning_about_a_mirror_git_ignores_shows_a_non_ascii_path_as_it_is() {
    let sandbox = setup("");
    let gitignore = sandbox.skill_repo().join(".gitignore");
    let mut rules = read(&gitignore);
    rules.push_str("projects/work/美国/\n");
    std::fs::write(&gitignore, &rules).unwrap();
    write(root(&sandbox).join("美国/AGENTS.md"), "# 美国\n");

    let (_, stderr) = run(adopt(&sandbox, ["--yes"]), 0);

    assert!(
        stderr.contains(&format!(
            "warning: git ignores projects/work/美国/AGENTS.md in the canon (.gitignore:{}:projects/work/美国/), so it never gets history; change that rule\n",
            rules.lines().count()
        )),
        "{stderr}"
    );
}
