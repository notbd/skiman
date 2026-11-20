//! The canon's git hooks (spec Hooks): pre-commit runs
//! `skiman check --staged` and fails closed; the post hooks run
//! `skiman sync --hook` and always exit 0. `init` installs them.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::failure::Failure;
use crate::fsutil::{absolute_lexical, write_atomic_executable};

/// The comment line that marks a hook file as skiman's.
pub const MARKER: &str = "# Managed by skiman: `skiman init` rewrites this file.";

pub const PRE_COMMIT: &str = "pre-commit";
/// Hooks that distribute after git changed the working tree.
pub const POST_HOOKS: [&str; 4] = ["post-commit", "post-merge", "post-checkout", "post-rewrite"];

/// Every hook skiman installs, pre-commit first.
pub fn hook_names() -> impl Iterator<Item = &'static str> {
    std::iter::once(PRE_COMMIT).chain(POST_HOOKS)
}

/// What `install` did.
#[derive(Debug, Default)]
pub struct Installed {
    /// Hook files written (absent before, or skiman's with other content).
    pub written: Vec<PathBuf>,
    /// Hook files that are not skiman's and lack the line to add, left unchanged.
    pub foreign: Vec<PathBuf>,
    /// `core.hooksPath` points away from the repo's own hooks folder: nothing
    /// was written. With the hooks there that lack the line to add.
    pub hooks_path_elsewhere: Option<(PathBuf, Vec<&'static str>)>,
    /// The canon is not in a git repo: nothing was written.
    pub not_a_git_repo: bool,
}

/// Installs every hook into the folder `git rev-parse --git-path hooks`
/// reports for the git repo at `repo_root`. Hook files that are not skiman's
/// are never touched, and nothing is written when `core.hooksPath` points elsewhere.
pub fn install(repo_root: &Path, binary: &Path) -> Result<Installed, Failure> {
    let mut installed = Installed::default();
    let Some((hooks_dir, own_dir)) = hooks_dirs(repo_root)? else {
        installed.not_a_git_repo = true;
        return Ok(installed);
    };
    if hooks_dir != own_dir {
        let lacking = hook_names()
            .filter(|name| {
                !std::fs::read(hooks_dir.join(name)).is_ok_and(|current| {
                    current == script(name, binary).as_bytes() || holds_line(&current, name, binary)
                })
            })
            .collect();
        installed.hooks_path_elsewhere = Some((hooks_dir, lacking));
        return Ok(installed);
    }
    let io_error = |path: &Path, e: std::io::Error| {
        Failure::operational(format!("cannot write git hook {}: {e}", path.display()))
    };
    std::fs::create_dir_all(&hooks_dir).map_err(|e| io_error(&hooks_dir, e))?;
    for name in hook_names() {
        let path = hooks_dir.join(name);
        let wanted = script(name, binary);
        match std::fs::read(&path) {
            Ok(current) if current == wanted.as_bytes() => continue,
            Ok(current) if !is_skimans(&current) => {
                if !holds_line(&current, name, binary) {
                    installed.foreign.push(path);
                }
                continue;
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(Failure::operational(format!(
                    "cannot read git hook {}: {e}",
                    path.display()
                )));
            }
        }
        write_atomic_executable(&path, wanted.as_bytes()).map_err(|e| io_error(&path, e))?;
        installed.written.push(path);
    }
    Ok(installed)
}

/// Where the canon's git hooks stand, for `doctor`.
#[derive(Debug)]
pub enum Status {
    /// The canon is not in a git repo.
    NotAGitRepo,
    /// Each hook file in the folder git runs hooks from, in [`hook_names`]
    /// order; `elsewhere` when `core.hooksPath` points away from the repo's
    /// own hooks folder, where `install` writes nothing.
    Hooks {
        dir: PathBuf,
        elsewhere: bool,
        hooks: Vec<(PathBuf, Hook)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hook {
    /// Exactly what `install` writes, and executable.
    Installed,
    /// Not skiman's, holds the line [`line_to_add`] gives, and executable;
    /// whether git reaches that line is not known.
    HoldsLine,
    /// What `install` writes, or holds the line, but not executable: git skips it.
    NotExecutable,
    Missing,
    /// skiman's (it has the marker) with other content, e.g. another binary.
    Outdated,
    /// Not skiman's and without the line: `install` leaves it alone.
    Foreign,
    Unreadable(String),
}

/// Compares every hook file with what [`install`] would write for `binary`,
/// and looks for the line to add by hand in the others. Read-only: no hook runs.
pub fn status(repo_root: &Path, binary: &Path) -> Result<Status, Failure> {
    let Some((hooks_dir, own_dir)) = hooks_dirs(repo_root)? else {
        return Ok(Status::NotAGitRepo);
    };
    let hooks = hook_names()
        .map(|name| {
            let path = hooks_dir.join(name);
            let hook = match std::fs::read(&path) {
                Ok(current) => {
                    let hook = if current == script(name, binary).as_bytes() {
                        Hook::Installed
                    } else if is_skimans(&current) {
                        Hook::Outdated
                    } else if holds_line(&current, name, binary) {
                        Hook::HoldsLine
                    } else {
                        Hook::Foreign
                    };
                    let executable = std::fs::metadata(&path)
                        .is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0);
                    match hook {
                        Hook::Installed | Hook::HoldsLine if !executable => Hook::NotExecutable,
                        hook => hook,
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Hook::Missing,
                Err(e) => Hook::Unreadable(e.to_string()),
            };
            (path, hook)
        })
        .collect();
    Ok(Status::Hooks {
        elsewhere: hooks_dir != own_dir,
        dir: hooks_dir,
        hooks,
    })
}

/// Whether a hook file holds the line [`line_to_add`] gives.
fn holds_line(content: &[u8], name: &str, binary: &Path) -> bool {
    let line = line_to_add(name, binary);
    content
        .split(|b| *b == b'\n')
        .any(|l| l.trim_ascii() == line.as_bytes())
}

fn is_skimans(content: &[u8]) -> bool {
    content
        .split(|b| *b == b'\n')
        .any(|line| line == MARKER.as_bytes())
}

/// The folder git runs hooks from, and the repo's own `hooks` folder; they
/// differ when `core.hooksPath` points elsewhere. `None` outside a git repo.
fn hooks_dirs(repo_root: &Path) -> Result<Option<(PathBuf, PathBuf)>, Failure> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-path", "hooks", "--git-common-dir"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| Failure::operational(format!("cannot run git (skiman requires git): {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    match (output.status.success(), lines.next(), lines.next()) {
        (true, Some(hooks), Some(common)) => Ok(Some((
            absolute_lexical(repo_root, Path::new(hooks)),
            absolute_lexical(repo_root, Path::new(common)).join("hooks"),
        ))),
        (false, ..) => Ok(None),
        _ => Err(Failure::operational(format!(
            "cannot find the git hooks folder of {}: {}",
            repo_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

/// The whole hook file skiman writes.
fn script(name: &str, binary: &Path) -> String {
    let binary = shell_quote(&binary.to_string_lossy());
    let body = if name == PRE_COMMIT {
        "if [ ! -x \"$skiman\" ]; then\n\
         \techo \"skiman: $skiman does not exist, so the commit is rejected; install skiman, then run \\`skiman init <canon>\\` again\" >&2\n\
         \texit 1\n\
         fi\n\
         exec \"$skiman\" check --staged\n"
    } else {
        "if [ -x \"$skiman\" ]; then\n\
         \t\"$skiman\" sync --hook\n\
         else\n\
         \techo \"skiman: $skiman does not exist, so the agent tools were not updated; install skiman, then run \\`skiman init <canon>\\` again\" >&2\n\
         fi\n\
         exit 0\n"
    };
    format!("#!/bin/sh\n{MARKER}\nskiman={binary}\n{body}")
}

/// The line to add by hand to a hook skiman cannot write.
pub fn line_to_add(name: &str, binary: &Path) -> String {
    let binary = shell_quote(&binary.to_string_lossy());
    if name == PRE_COMMIT {
        format!("{binary} check --staged || exit 1")
    } else {
        format!("{binary} sync --hook || true")
    }
}

pub(crate) fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
