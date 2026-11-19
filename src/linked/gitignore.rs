//! The `/linked/` line of the canon's `.gitignore` (spec "`.gitignore`"):
//! `init` and `linked add` write it, so the links in `linked/` never reach
//! the canon's history.

use std::path::Path;

use crate::failure::Failure;
use crate::fsutil::{rewrite_atomic, write_atomic};
use crate::git;

/// The line, as written.
pub const LINE: &str = "/linked/";

/// What [`ignore`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The line was appended (the file created when missing).
    Added,
    /// `linked/` is ignored already, or the line is there.
    Present,
    /// `.gitignore` is a link: nothing was written.
    Link,
}

/// Appends `/linked/` to the canon's `.gitignore` unless git already
/// ignores `linked/` by the canon's own rules (`check-ignore` with the
/// folder's slash, so the rule counts before sync has made it) or the
/// file's last non-empty line is `/linked/`. In a canon that is not a git
/// repo there is no check: the line is appended unless any line is
/// `/linked/`. A `.gitignore` that is a link is left alone.
pub fn ignore(root: &Path) -> Result<Outcome, Failure> {
    let path = root.join(".gitignore");
    let cannot_read =
        |e: std::io::Error| Failure::operational(format!("cannot read {}: {e}", path.display()));
    let (text, permissions) = match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_symlink() => return Ok(Outcome::Link),
        Ok(meta) => (
            std::fs::read_to_string(&path).map_err(cannot_read)?,
            Some(meta.permissions()),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), None),
        Err(e) => return Err(cannot_read(e)),
    };
    let present = match check_ignore(root) {
        Some(true) => true,
        Some(false) => text.lines().rev().find(|l| !l.trim().is_empty()) == Some(LINE),
        None => text.lines().any(|l| l == LINE),
    };
    if present {
        return Ok(Outcome::Present);
    }
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let new = format!("{text}{separator}{LINE}\n");
    match permissions {
        Some(permissions) => rewrite_atomic(&path, new.as_bytes(), permissions),
        None => write_atomic(&path, new.as_bytes()),
    }
    .map_err(|e| Failure::operational(format!("cannot write {}: {e}", path.display())))?;
    Ok(Outcome::Added)
}

/// Whether git ignores `linked/` in the canon at `root` by the canon's own
/// ignore files (the user's global excludes file left out); `None` when
/// the canon is not a git repo or git cannot tell.
pub fn check_ignore(root: &Path) -> Option<bool> {
    let status = git::query_command(root)
        .args([
            "-c",
            "core.excludesFile=/dev/null",
            "check-ignore",
            "-q",
            "--no-index",
            "linked/",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}
