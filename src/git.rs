//! Running git safely (spec Source fetching): never through a shell, with
//! `--` before URLs and paths, with protocols that run commands and hooks
//! switched off, and without the git environment of a calling git process
//! (a hook may set `GIT_DIR` or `GIT_INDEX_FILE`).
//!
//! [`fetch_command`] is the FETCH profile: the user's config stays in effect
//! (credentials, SSH config, `insteadOf`). [`merge_command`] is the MERGE
//! profile for the temporary merge repository: it drops the user's config,
//! attributes and environment as well, so no merge driver, filter or hook of
//! the user's can run and no setting changes the merge result.

use std::path::Path;
use std::process::{Command, Output};

use crate::failure::Failure;

/// `-c` settings every git call gets: no `ext::`/`fd::` transports, no hooks,
/// no fsmonitor, and no background maintenance in the cache.
const SAFE_CONFIG: &[&str] = &[
    "protocol.ext.allow=never",
    "protocol.fd.allow=never",
    "core.hooksPath=/dev/null",
    "core.fsmonitor=false",
    "gc.auto=0",
    "maintenance.auto=false",
];

/// Variables of a calling git process that would point git at another
/// repository, index or object store (`git rev-parse --local-env-vars`,
/// minus the command-scope config variables, which are user config).
const LOCAL_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// The oldest git skiman works with: `merge-file --object-id` (2.43) for
/// update's merge. `doctor` reports an older git.
pub const MIN_VERSION: (u32, u32) = (2, 43);

/// The installed git's version as `major.minor.patch` (from `git --version`,
/// e.g. `git version 2.39.5 (Apple Git-154)` gives `2.39.5`) with its
/// first two numbers. `Err` when git cannot be run or prints something else.
pub fn installed_version() -> Result<(String, (u32, u32)), String> {
    let output = Command::new("git")
        .arg("--version")
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout
        .trim()
        .strip_prefix("git version ")
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or_else(|| format!("`git --version` printed {:?}", stdout.trim()))?;
    let mut numbers = version.split('.').map(|n| n.parse::<u32>());
    match (numbers.next(), numbers.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => Ok((version.to_owned(), (major, minor))),
        _ => Err(format!("`git --version` printed {:?}", stdout.trim())),
    }
}

/// How git may ask for credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompts {
    /// A terminal is attached: git and ssh may prompt.
    Allowed,
    /// No terminal: fail instead of waiting for an answer
    /// (`GIT_TERMINAL_PROMPT=0`, ssh `BatchMode=yes`, no askpass).
    Never,
}

/// A git command in the FETCH profile on the bare repository `git_dir`.
pub fn fetch_command(git_dir: &Path, prompts: Prompts) -> Command {
    let mut cmd = Command::new("git");
    for var in LOCAL_ENV_VARS {
        cmd.env_remove(var);
    }
    cmd.env("GIT_DIR", git_dir);
    for setting in SAFE_CONFIG {
        cmd.args(["-c", setting]);
    }
    if prompts == Prompts::Never {
        cmd.args(["-c", "credential.interactive=false"]);
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.env(
            "GIT_SSH_COMMAND",
            format!("{} -o BatchMode=yes", effective_ssh_command(git_dir)),
        );
    }
    cmd
}

/// A read-only git query in the user's repository at `dir` (`git -C <dir>`):
/// the user's config stays in effect (ignore files, `core.excludesFile`), the
/// calling git process's environment does not, and [`SAFE_CONFIG`] applies.
pub fn query_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    for var in LOCAL_ENV_VARS {
        cmd.env_remove(var);
    }
    for setting in SAFE_CONFIG {
        cmd.args(["-c", setting]);
    }
    cmd.arg("-C").arg(dir);
    cmd
}

/// Settings the MERGE profile adds: the user's global attributes file is not
/// covered by `GIT_CONFIG_GLOBAL` (git-fetch-and-merge.md 1.3).
const MERGE_CONFIG: &[&str] = &["core.attributesFile=/dev/null"];

/// A git command in the MERGE profile on the temporary bare repository
/// `git_dir`. Nothing is inherited from the environment except `PATH`;
/// `HOME` and `XDG_CONFIG_HOME` point at `home` (a folder of the temporary
/// merge area), global and system config are off, and commits get a fixed
/// identity and date.
pub fn merge_command(git_dir: &Path, home: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_DIR", git_dir)
        .env("LC_ALL", "C");
    for (who, value) in [("NAME", "skiman"), ("EMAIL", "skiman@invalid")] {
        cmd.env(format!("GIT_AUTHOR_{who}"), value)
            .env(format!("GIT_COMMITTER_{who}"), value);
    }
    cmd.env("GIT_AUTHOR_DATE", "@0 +0000")
        .env("GIT_COMMITTER_DATE", "@0 +0000");
    for setting in SAFE_CONFIG.iter().chain(MERGE_CONFIG) {
        cmd.args(["-c", setting]);
    }
    cmd
}

/// The ssh command git would use, in git's own order: `GIT_SSH_COMMAND`,
/// `core.sshCommand`, `GIT_SSH` (a program path, quoted for the shell git
/// runs `GIT_SSH_COMMAND` with), then `ssh`.
fn effective_ssh_command(git_dir: &Path) -> String {
    if let Some(command) = std::env::var_os("GIT_SSH_COMMAND").filter(|c| !c.is_empty()) {
        return command.to_string_lossy().into_owned();
    }
    let mut config = Command::new("git");
    for var in LOCAL_ENV_VARS {
        config.env_remove(var);
    }
    config
        .env("GIT_DIR", git_dir)
        .args(["config", "--get", "core.sshCommand"]);
    if let Ok(output) = config.output()
        && output.status.success()
    {
        let command = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !command.is_empty() {
            return command;
        }
    }
    if let Some(program) = std::env::var_os("GIT_SSH").filter(|p| !p.is_empty()) {
        return format!("'{}'", program.to_string_lossy().replace('\'', r"'\''"));
    }
    "ssh".to_owned()
}

/// Runs `cmd`; a failure to start git or a non-zero exit is an operational
/// error (exit 4) naming `what` and quoting git's message.
pub fn run(mut cmd: Command, what: &str) -> Result<Vec<u8>, Failure> {
    let output = cmd
        .output()
        .map_err(|e| Failure::operational(format!("cannot run git (skiman requires git): {e}")))?;
    check(&output, what)?;
    Ok(output.stdout)
}

fn check(output: &Output, what: &str) -> Result<(), Failure> {
    if output.status.success() {
        return Ok(());
    }
    Err(Failure::operational(format!(
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}
