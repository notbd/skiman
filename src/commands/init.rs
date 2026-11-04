//! `skiman init <path>`: create or reuse a skill repo, write the user config,
//! then sync. When the user config pointed to another skill repo, that sync
//! treats the previous repo's links as owned. Before that sync it installs the
//! git hooks, and finally it adds or removes the session-start hook entries.

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
use crate::git_hooks;
use crate::manifest::new_manifest_text;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::session_hooks;
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
    } else {
        ignore_machine_state(&repo)?;
    }
    let previous_repo = UserConfig::load(&paths)
        .ok()
        .map(|config| config.repo.canonicalize().unwrap_or(config.repo))
        .filter(|previous| previous != repo.root());
    let binary = invoked_binary(&cwd);
    UserConfig {
        repo: path,
        binary: binary.clone(),
    }
    .save(&paths)?;
    install_git_hooks(&repo, &binary)?;
    let (exit, report) = sync::run_with(&paths, &repo, &lock, Output::Normal, previous_repo);
    hint_not_owned(&report, &repo);
    // Settings files are outside the skill repo: do not hold the repo lock
    // while the preview waits for an answer.
    drop(lock);
    Ok(exit.combine(install_session_hooks(&paths, &binary, session_hooks)))
}

/// `--session-hooks <ids>` adds entries, `none` removes skiman's, and no flag
/// (only possible with a terminal) previews the entries and asks.
fn install_session_hooks(paths: &Paths, binary: &Path, session_hooks: Option<&str>) -> Exit {
    let request = match session_hooks {
        Some("none") => session_hooks::Request::Remove,
        Some(ids) => session_hooks::Request::Add(ids),
        None => session_hooks::Request::Ask,
    };
    session_hooks::apply(paths.home(), binary, request)
}

/// Story 5: content skiman does not own in the tool directories or the
/// instruction locations. With no skills in the skill repo yet, the way in is
/// `skiman import`; otherwise `skiman adopt` for each foreign skill, and an
/// instruction file is moved away by hand (skiman never replaces it).
fn hint_not_owned(report: &sync::Report, repo: &SkillRepo) {
    let foreign = report
        .items
        .iter()
        .any(|item| matches!(item.kind, ItemKind::Foreign { .. }));
    let refused = report
        .items
        .iter()
        .any(|item| item.kind == ItemKind::RefusedLocation);
    if (foreign || refused) && !has_skills(repo) {
        eprintln!(
            "hint: the skill repo has no skills yet and the tool directories or instruction locations hold content skiman does not own; bring your existing setup in with `skiman import` (see `skiman import --guide`)"
        );
        return;
    }
    if foreign {
        eprintln!(
            "hint: the tool directories hold foreign skills; bring each one into the skill repo with `skiman adopt <name> --from <tool id>`"
        );
    }
    if refused {
        eprintln!(
            "hint: move each instruction file listed above away by hand (copy what you want to keep into {} first), then run `skiman sync`",
            repo.shared_body().display()
        );
    }
}

/// Installs the git hooks and tells the user what to add by hand where
/// skiman left a hook alone.
fn install_git_hooks(repo: &SkillRepo, binary: &Path) -> Result<(), Failure> {
    let installed = git_hooks::install(repo.root(), binary)?;
    if installed.not_a_git_repo {
        eprintln!(
            "warning: {} is not in a git repository, so skiman installed no git hooks; run `git init` there, then `skiman init` again",
            repo.root().display()
        );
    }
    for path in &installed.written {
        println!("installed git hook {}", path.display());
    }
    for path in &installed.foreign {
        let name = path.file_name().expect("hook has a name").to_string_lossy();
        eprintln!(
            "warning: {} is not skiman's git hook, so skiman left it unchanged; add this line to it:\n  {}",
            path.display(),
            git_hooks::line_to_add(&name, binary)
        );
    }
    if let Some(dir) = &installed.hooks_path_elsewhere {
        eprintln!(
            "warning: core.hooksPath points to {}, so skiman installed no git hooks; add these lines to the hooks there:",
            dir.display()
        );
        for name in git_hooks::hook_names() {
            eprintln!("  {name}:\n    {}", git_hooks::line_to_add(name, binary));
        }
    }
    Ok(())
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

/// A reused skill repo gets the `.gitignore` entry for `.skiman/` when no
/// line ignores it yet (the file is created when missing); nothing else of
/// it changes.
fn ignore_machine_state(repo: &SkillRepo) -> Result<(), Failure> {
    let path = repo.root().join(".gitignore");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(Failure::operational(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    if text.lines().any(|line| {
        matches!(
            line.trim(),
            ".skiman" | ".skiman/" | "/.skiman" | "/.skiman/"
        )
    }) {
        return Ok(());
    }
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::write(&path, format!("{text}{separator}.skiman/\n"))
        .map_err(|e| Failure::operational(format!("cannot write {}: {e}", path.display())))?;
    println!("added .skiman/ to {}", path.display());
    Ok(())
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
