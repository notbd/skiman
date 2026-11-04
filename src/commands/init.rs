//! `skiman init <path>`: create or reuse a skill repo, write the user config,
//! then sync. When the user config pointed to another skill repo, that sync
//! treats the previous repo's links as owned. Git hooks are #8;
//! session-start entries are #9.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Exit;
use crate::commands::has_terminal;
use crate::config::UserConfig;
use crate::failure::{Failure, finish};
use crate::fsutil::absolute_lexical;
use crate::manifest::new_manifest_text;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::sync::{self, ItemKind, Output};
use crate::tools::TOOLS;

/// `session_hooks` is the validated `--session-hooks` value (`none` or tool ids).
pub fn run(path: &Path, session_hooks: Option<&str>) -> Exit {
    finish(init(path, session_hooks))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// Missing or empty: create the layout.
    Create,
    /// Holds `skiman.toml`: reuse it as it is.
    Reuse,
}

fn init(path: &Path, session_hooks: Option<&str>) -> Result<Exit, Failure> {
    if session_hooks.is_none() && !has_terminal() {
        return Err(Failure::usage(
            "missing flag without a terminal: --session-hooks <ids|none>",
        ));
    }
    let paths = Paths::from_env()?;
    let cwd = std::env::current_dir()
        .map_err(|e| Failure::operational(format!("cannot read the current directory: {e}")))?;
    let path = absolute_lexical(&cwd, path);
    let target = inspect(&path)?;

    if target == Target::Create {
        git_init(&path)?;
    }
    let repo = SkillRepo::at(&path)?;
    let lock = RepoLock::wait(&repo)?;
    if target == Target::Create {
        write_layout(&repo, paths.home())
            .map_err(|e| Failure::operational(format!("cannot create the skill repo: {e}")))?;
        println!("created skill repo {}", path.display());
    }
    let previous_repo = UserConfig::load(&paths)
        .ok()
        .map(|config| config.repo.canonicalize().unwrap_or(config.repo))
        .filter(|previous| previous != repo.root());
    UserConfig {
        repo: path,
        binary: invoked_binary(&cwd),
    }
    .save(&paths)?;
    let (exit, report) = sync::run_with(&paths, &repo, &lock, Output::Normal, previous_repo);
    let foreign = report
        .items
        .iter()
        .any(|item| matches!(item.kind, ItemKind::Foreign { .. }));
    if foreign && has_skills(&repo) {
        eprintln!(
            "hint: the tool directories hold foreign skills; bring each one into the skill repo with `skiman adopt <name> --from <tool id>`"
        );
    } else if foreign {
        eprintln!(
            "hint: the tool directories hold foreign skills and the skill repo has none yet; bring your existing setup in with `skiman import` (see `skiman import --guide`)"
        );
    }
    Ok(exit)
}

/// Whether any folder under `skills/` holds a `SKILL.md`.
fn has_skills(repo: &SkillRepo) -> bool {
    repo.skill_folder_names()
        .unwrap_or_default()
        .iter()
        .any(|name| repo.skill_dir(name).join("SKILL.md").is_file())
}

fn inspect(path: &Path) -> Result<Target, Failure> {
    let mut entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Target::Create),
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => {
            return Err(Failure::refused(format!(
                "{} is not a folder",
                path.display()
            )));
        }
        Err(e) => {
            return Err(Failure::operational(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    if std::fs::symlink_metadata(path.join("skiman.toml")).is_ok() {
        Ok(Target::Reuse)
    } else if entries.next().is_none() {
        Ok(Target::Create)
    } else {
        Err(Failure::refused(format!(
            "{} is not empty and holds no skiman.toml; choose an empty or missing folder, or an existing skill repo",
            path.display()
        )))
    }
}

/// `git init` with no commit; creates `path` when missing.
fn git_init(path: &Path) -> Result<(), Failure> {
    let output = Command::new("git")
        .args(["init", "--quiet", "--"])
        .arg(path)
        .output()
        .map_err(|e| Failure::operational(format!("cannot run git (skiman requires git): {e}")))?;
    if !output.status.success() {
        return Err(Failure::operational(format!(
            "git init {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// `skills/`, an empty `instructions/shared.md`, `skiman.toml` listing the
/// tools present right now, and the `.gitignore` entry for `.skiman/`.
fn write_layout(repo: &SkillRepo, home: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(repo.skills_dir())?;
    let shared = repo.shared_body();
    std::fs::create_dir_all(shared.parent().expect("shared body has a parent"))?;
    std::fs::write(&shared, "")?;
    let present: Vec<&str> = TOOLS
        .iter()
        .filter(|tool| tool.is_present(home))
        .map(|tool| tool.id)
        .collect();
    std::fs::write(repo.manifest_file(), new_manifest_text(&present))?;
    std::fs::write(repo.root().join(".gitignore"), ".skiman/\n")
}

/// The path skiman was invoked through: `argv[0]` made absolute, or resolved
/// against `PATH` when it has no `/`. Symlinks are not resolved, so the hooks
/// keep working when a package manager swaps the binary behind the link.
fn invoked_binary(cwd: &Path) -> PathBuf {
    let current_exe = || std::env::current_exe().unwrap_or_default();
    let Some(argv0) = std::env::args_os().next() else {
        return current_exe();
    };
    if argv0.as_bytes().contains(&b'/') {
        return absolute_lexical(cwd, Path::new(&argv0));
    }
    find_on_path(&argv0)
        .map(|found| absolute_lexical(cwd, &found))
        .unwrap_or_else(current_exe)
}

fn find_on_path(program: &OsStr) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}
