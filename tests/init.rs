//! `skiman init <path>`: create or reuse a skill repo, write the user config, first sync.

mod support;

use snapbox::str;
use support::{Sandbox, Tool, ToolSetup};

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
error: [..]/skill-repo is not empty and holds no skiman.toml; choose an empty or missing folder, or an existing skill repo

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
