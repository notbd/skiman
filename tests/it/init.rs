//! `skiman init <path>`: create or reuse a skill repo, write the user config, first sync.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use snapbox::str;

use crate::support::{Sandbox, Tool, ToolSetup};

fn read(path: impl AsRef<std::path::Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

#[test]
fn init_on_a_missing_path_creates_the_skill_repo_layout_and_the_user_config() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Antigravity, ToolSetup::Absent)
        .build();
    let repo = sandbox.skill_repo();

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .assert()
        .code(0);

    assert!(repo.join("skills").is_dir());
    assert_eq!(read(repo.join("instructions/shared.md")), "");
    assert!(
        read(repo.join(".gitignore"))
            .lines()
            .any(|l| l == ".skiman/")
    );
    assert_eq!(
        read(repo.join("skiman.toml")),
        "[tools]\nenabled = [\"claude-code\", \"codex\"]\n"
    );
    assert_eq!(
        sandbox.git(&repo, ["rev-parse", "--is-inside-work-tree"]),
        "true"
    );
    let no_commits = sandbox
        .std_command("git")
        .current_dir(&repo)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .status()
        .unwrap();
    assert!(!no_commits.success(), "init must not commit");

    let config = read(sandbox.config_home().join("skiman/config.toml"));
    assert_eq!(
        config,
        format!(
            "repo = \"{}\"\nbinary = \"{}\"\n",
            repo.display(),
            env!("CARGO_BIN_EXE_skiman")
        )
    );
}

#[test]
fn init_records_the_binary_as_found_on_path_without_resolving_symlinks() {
    let sandbox = Sandbox::new();
    let bin = sandbox.root().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_skiman"), bin.join("skiman")).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());

    sandbox
        .command("skiman")
        .env("PATH", path)
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);

    let config = read(sandbox.config_home().join("skiman/config.toml"));
    assert!(
        config.contains(&format!("binary = \"{}\"\n", bin.join("skiman").display())),
        "{config}"
    );
}

#[test]
fn init_on_an_empty_folder_creates_the_layout_there() {
    let sandbox = Sandbox::new();
    let repo = sandbox.skill_repo();
    std::fs::create_dir(&repo).unwrap();

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .assert()
        .code(0);

    assert_eq!(
        read(repo.join("skiman.toml")),
        "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n"
    );
}

#[test]
fn init_on_a_clone_of_an_empty_repo_creates_the_layout_and_keeps_its_git_folder() {
    let sandbox = Sandbox::new();
    sandbox.git(sandbox.root(), ["init", "--quiet", "--bare", "origin.git"]);
    sandbox.git(
        sandbox.root(),
        ["clone", "--quiet", "origin.git", "skill-repo"],
    );
    let repo = sandbox.skill_repo();
    let git_config = read(repo.join(".git/config"));
    let head = read(repo.join(".git/HEAD"));

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
created skill repo [..]/skill-repo
...
"#]]);

    assert!(repo.join("skills").is_dir());
    assert_eq!(read(repo.join(".gitignore")), ".skiman/\n");
    assert_eq!(read(repo.join(".git/config")), git_config);
    assert_eq!(read(repo.join(".git/HEAD")), head);
}

#[test]
fn init_on_a_folder_with_skiman_toml_reuses_it_and_leaves_tools_untouched() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Codex, ToolSetup::Absent)
        .build();
    let repo = sandbox.skill_repo();
    std::fs::create_dir_all(repo.join("skills")).unwrap();
    let manifest = "# my tools\n[tools]\nenabled = [ \"codex\",\"claude-code\" ]  # both\n";
    std::fs::write(repo.join("skiman.toml"), manifest).unwrap();

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .assert()
        .code(0);

    assert_eq!(read(repo.join("skiman.toml")), manifest);
    assert!(sandbox.config_home().join("skiman/config.toml").is_file());
}

#[test]
fn init_on_a_non_empty_folder_without_skiman_toml_is_refused_and_changes_nothing() {
    let sandbox = Sandbox::new();
    let repo = sandbox.skill_repo();
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("notes.txt"), "mine").unwrap();

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .assert()
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: [..]/skill-repo is not empty and holds no skiman.toml; choose an empty or missing folder, a folder that holds only `.git` (a fresh `git init` or a clone of an empty repo), or an existing skill repo

"#]]);

    let entries: Vec<_> = std::fs::read_dir(&repo).unwrap().collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(read(repo.join("notes.txt")), "mine");
    assert!(!sandbox.config_home().join("skiman").exists());
    assert!(
        std::fs::read_dir(sandbox.tool_dir(Tool::ClaudeCode))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn init_without_a_terminal_requires_session_hooks_and_writes_nothing() {
    let sandbox = Sandbox::new();

    sandbox
        .skiman()
        .arg("init")
        .arg(sandbox.skill_repo())
        .assert()
        .code(2)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: missing flag without a terminal: --session-hooks <ids|none>

"#]]);

    assert!(!sandbox.skill_repo().exists());
    assert!(!sandbox.config_home().join("skiman").exists());
}

#[test]
fn init_refuses_an_unknown_session_hooks_value_as_a_usage_error() {
    let sandbox = Sandbox::new();

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "antigravity"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(2);

    assert!(!sandbox.skill_repo().exists());
}

/// Spec Skill repo layout: `init` writes the `.gitignore` entry for
/// `.skiman/`, also in a reused repo that lacks it; nothing else changes.
#[test]
fn init_on_a_reused_repo_adds_the_skiman_gitignore_entry_once() {
    let sandbox = Sandbox::new();
    let repo = sandbox.skill_repo();
    std::fs::create_dir_all(&repo).unwrap();
    let manifest = "[tools]\nenabled = []\n";
    std::fs::write(repo.join("skiman.toml"), manifest).unwrap();
    std::fs::write(repo.join(".gitignore"), "target").unwrap();
    sandbox.git(&repo, ["init", "--quiet"]);
    let init = || {
        sandbox
            .skiman()
            .args(["init", "--session-hooks", "none"])
            .arg(&repo)
            .assert()
            .code(0)
    };

    init().stdout_eq(str![[r#"
added .skiman/ to [..]/skill-repo/.gitignore
...
"#]]);
    assert_eq!(read(repo.join(".gitignore")), "target\n.skiman/\n");
    assert_eq!(read(repo.join("skiman.toml")), manifest);
    assert_eq!(
        sandbox.git(&repo, ["status", "--porcelain", "--untracked-files=all"]),
        "?? .gitignore\n?? skiman.toml"
    );

    let out = init().get_output().stdout.clone();
    assert!(!String::from_utf8_lossy(&out).contains("added .skiman/"));
    assert_eq!(read(repo.join(".gitignore")), "target\n.skiman/\n");

    std::fs::remove_file(repo.join(".gitignore")).unwrap();
    init();
    assert_eq!(read(repo.join(".gitignore")), ".skiman/\n");
}

/// A skill repo `init` reuses: `skiman.toml` in a git repo, no `.gitignore`.
fn reused_repo(sandbox: &Sandbox) -> PathBuf {
    let repo = sandbox.skill_repo();
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("skiman.toml"), "[tools]\nenabled = []\n").unwrap();
    sandbox.git(&repo, ["init", "--quiet"]);
    repo
}

fn init_reused(sandbox: &Sandbox) -> snapbox::cmd::OutputAssert {
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
}

#[test]
fn init_swaps_in_a_new_gitignore_that_keeps_its_mode_and_leaves_a_hard_link_with_the_old_content() {
    let sandbox = Sandbox::new();
    let repo = reused_repo(&sandbox);
    let gitignore = repo.join(".gitignore");
    std::fs::write(&gitignore, "target\n").unwrap();
    std::fs::set_permissions(&gitignore, std::fs::Permissions::from_mode(0o640)).unwrap();
    let hard_link = sandbox.root().join("gitignore-hard-link");
    std::fs::hard_link(&gitignore, &hard_link).unwrap();

    init_reused(&sandbox).code(0);

    assert_eq!(read(&gitignore), "target\n.skiman/\n");
    assert_eq!(
        read(&hard_link),
        "target\n",
        "the new content was written into the old file"
    );
    let mode = std::fs::metadata(&gitignore).unwrap().permissions().mode();
    assert_eq!(mode & 0o7777, 0o640);
}

#[test]
fn init_adds_the_skiman_entry_to_the_file_a_linked_gitignore_points_to_and_keeps_the_link() {
    let sandbox = Sandbox::new();
    let repo = reused_repo(&sandbox);
    let dotfiles = sandbox.root().join("dotfiles");
    std::fs::create_dir(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("gitignore"), "target\n").unwrap();
    std::os::unix::fs::symlink(dotfiles.join("gitignore"), repo.join(".gitignore")).unwrap();

    init_reused(&sandbox).code(0);

    assert_eq!(
        std::fs::read_link(repo.join(".gitignore")).unwrap(),
        dotfiles.join("gitignore")
    );
    assert_eq!(read(dotfiles.join("gitignore")), "target\n.skiman/\n");
}

#[test]
fn init_refuses_a_gitignore_that_links_to_a_missing_file_and_does_not_create_that_file() {
    let sandbox = Sandbox::new();
    let repo = reused_repo(&sandbox);
    let dotfiles = sandbox.root().join("dotfiles");
    std::fs::create_dir(&dotfiles).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("gitignore"), repo.join(".gitignore")).unwrap();

    let assert = init_reused(&sandbox);

    assert!(
        std::fs::symlink_metadata(dotfiles.join("gitignore")).is_err(),
        "init created the file the link points to"
    );
    assert_eq!(
        std::fs::read_link(repo.join(".gitignore")).unwrap(),
        dotfiles.join("gitignore")
    );
    assert.code(1).stdout_eq("").stderr_eq(str![[r#"
error: [..]/skill-repo/.gitignore is a link to a missing file; create that file or remove the link, then run `skiman init` again

"#]]);
}
