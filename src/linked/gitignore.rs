//! The lines of the canon's `.gitignore` (spec "`.gitignore`"): `init`
//! writes `.skiman/` and `/linked/`, `linked add` writes `/linked/`, so
//! machine state and the links in `linked/` never reach the canon's history.

use std::path::Path;
use std::process::Stdio;

use crate::failure::Failure;
use crate::fsutil::{rewrite_atomic, write_atomic};
use crate::git;

/// The line for `linked/`, as written.
pub const LINE: &str = "/linked/";
/// The line for `.skiman/`, as written.
pub const SKIMAN_LINE: &str = ".skiman/";

/// What [`ignore`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The line was appended (the file created when missing).
    Added,
    /// git ignores the folder already, or the line counts as there.
    Present,
    /// `.gitignore` is a link: nothing was written.
    Link,
}

/// Appends `line` (a folder line, e.g. [`LINE`]) to the canon's
/// `.gitignore` unless git already ignores that folder by the canon's own
/// rules (`check-ignore` with the folder's slash, so the rule counts before
/// the folder exists) or the file's last non-empty line is `line` (an
/// earlier one may be undone by a later rule). When git cannot tell, or the
/// canon is not a git repo, the last line that is `line` or `!line` decides.
/// A `.gitignore` that is a link is left alone: git does not read it.
pub fn ignore(root: &Path, line: &str) -> Result<Outcome, Failure> {
    let path = root.join(".gitignore");
    let cannot_read =
        |e: std::io::Error| Failure::operational(format!("cannot read {}: {e}", path.display()));
    let ignored = check_ignore(root, line.trim_start_matches('/'));
    if ignored == Ignored::Yes {
        return Ok(Outcome::Present);
    }
    let (text, permissions) = match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_symlink() => return Ok(Outcome::Link),
        Ok(meta) => (
            std::fs::read_to_string(&path).map_err(cannot_read)?,
            Some(meta.permissions()),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), None),
        Err(e) => return Err(cannot_read(e)),
    };
    let present = match ignored {
        Ignored::No => text.lines().rev().find(|l| !l.trim().is_empty()) == Some(line),
        // The last line that names the folder, either way.
        _ => text.lines().rfind(|l| l.trim_start_matches('!') == line) == Some(line),
    };
    if present {
        return Ok(Outcome::Present);
    }
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let new = format!("{text}{separator}{line}\n");
    match permissions {
        Some(permissions) => rewrite_atomic(&path, new.as_bytes(), permissions),
        None => write_atomic(&path, new.as_bytes()),
    }
    .map_err(|e| Failure::operational(format!("cannot write {}: {e}", path.display())))?;
    Ok(Outcome::Added)
}

/// What git says about a folder of the canon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ignored {
    Yes,
    No,
    /// The canon is not a git repo, or git cannot run: nothing to ask.
    NotARepo,
    /// The canon is a git repo but git cannot tell; its message.
    Failed(String),
}

/// Whether git ignores `folder` (relative, with its trailing slash) in the
/// canon at `root` by the canon's own ignore files (the user's global
/// excludes file left out).
pub fn check_ignore(root: &Path, folder: &str) -> Ignored {
    let Ok(output) = git::query_command(root)
        .args([
            "-c",
            "core.excludesFile=/dev/null",
            "check-ignore",
            "-q",
            "--no-index",
            folder,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
    else {
        return Ignored::NotARepo;
    };
    match output.status.code() {
        Some(0) => Ignored::Yes,
        Some(1) => Ignored::No,
        _ if !is_repo(root) => Ignored::NotARepo,
        _ => Ignored::Failed(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

/// Whether `root` is in a git repo.
fn is_repo(root: &Path) -> bool {
    git::query_command(root)
        .args(["rev-parse", "--git-dir"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
