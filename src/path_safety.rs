//! Path safety (spec Source fetching): the checks every piece of candidate
//! content passes before anything of it is written, whatever its source.
//! They block even with `--yes`.
//!
//! One entry point, [`check`], on any [`Tree`]: a git listing
//! ([`crate::snapshot::Snapshot::read`]), a local folder
//! ([`crate::snapshot::Snapshot::read_local`]), an
//! [`crate::tree::Overlay`] of a command's output (update's merged folder),
//! a [`crate::tree::WorkingTree`] of an import or adopt folder.
//!
//! Refused: absolute paths, empty, `.`, `..` and `.git` components, an entry
//! whose parent folder is not listed (a crafted git name holding `/`), names
//! that are not UTF-8, any symlink, any special file (FIFO, socket, device),
//! case and Unicode-normalisation (NFC) collisions, Git LFS pointers and the
//! size limits of a skill. For [`Content::Upstream`] also conflict-record
//! names and skiman conflict-marker lines (spec Merge semantics).
//!
//! Names and kinds are checked first; file content is read only when they
//! pass, so an oversized file is never read.

use std::collections::{BTreeMap, BTreeSet};

use unicode_normalization::UnicodeNormalization;

use crate::failure::Failure;
use crate::rules::{CONFLICT_MARKERS, CONFLICT_RECORD_SUFFIXES, CONFLICTS_DIR, LIMITS};
use crate::tree::{Kind, Tree, basename, is_ignored};

/// What the checked content is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// Content from an upstream (git or local folder) or another outside
    /// source: conflict-record names and conflict-marker lines are refused too.
    Upstream,
    /// A command's own output, which may hold conflict markers and records
    /// (update's merged skill folder).
    Output,
}

const MIB: u64 = 1024 * 1024;
/// The Git LFS spec: a pointer file is smaller than this.
const LFS_POINTER_MAX: usize = 1024;
const LFS_VERSIONS: &[&str] = &[
    "version https://git-lfs.github.com/spec/v1",
    "version https://hawser.github.com/spec/v1",
];

/// Refuses (exit 1) the folder `dir` of `tree` when anything in it is unsafe
/// to write. `origin` names the content in the message (a URL, a folder);
/// every problem is one line naming its path relative to `dir`. Ignored names
/// (`.DS_Store`, `Thumbs.db`) are never written and so not checked.
pub fn check(tree: &dyn Tree, dir: &str, origin: &str, content: Content) -> Result<(), Failure> {
    let io = |e: std::io::Error| Failure::operational(format!("cannot read {origin}: {e}"));
    let skip = if dir.is_empty() { 0 } else { dir.len() + 1 };
    let entries = tree.walk(dir).map_err(io)?;
    let folders: BTreeSet<&str> = entries
        .iter()
        .filter(|e| e.kind == Kind::Dir)
        .map(|e| &e.path[skip..])
        .collect();

    let mut problems = Vec::new();
    let mut folded: BTreeMap<String, &str> = BTreeMap::new();
    let mut files = Vec::new();
    let mut bytes = 0u64;
    for entry in &entries {
        let path = &entry.path[skip..];
        if entry.kind != Kind::Dir && is_ignored(path) {
            continue;
        }
        if path.contains('\u{FFFD}') {
            problems.push(format!("a path that is not valid UTF-8: {path:?}"));
            continue;
        }
        let parent_listed = path
            .rsplit_once('/')
            .is_none_or(|(parent, _)| folders.contains(parent));
        if !parent_listed || !path.split('/').all(is_safe_component) {
            problems.push(format!(
                "an unsafe path {path:?} (absolute paths and empty, `.`, `..` and `.git` names are not allowed)"
            ));
            continue;
        }
        let key = path.nfc().collect::<String>().to_lowercase();
        if let Some(other) = folded.insert(key, path) {
            problems.push(format!(
                "{other} and {path}, which are the same path on a case- or normalisation-insensitive file system"
            ));
        }
        let name = basename(path);
        if content == Content::Upstream
            && ((entry.kind == Kind::Dir && name == CONFLICTS_DIR)
                || CONFLICT_RECORD_SUFFIXES.iter().any(|s| name.ends_with(s)))
        {
            problems.push(format!(
                "{path}, a name skiman keeps for conflict records (`*.skiman-upstream`, `*.skiman-upstream-deleted`, `.skiman-conflicts`)"
            ));
        }
        match entry.kind {
            Kind::Dir => {}
            Kind::Symlink => problems.push(format!(
                "a symlink at {path}; symlinks are not allowed in skills"
            )),
            Kind::Special => problems.push(format!(
                "{path}, which is not a regular file (a FIFO, socket or device)"
            )),
            Kind::File { size, .. } => {
                if size > LIMITS.file_bytes {
                    problems.push(format!(
                        "{path} of {size} bytes; a file in a skill may be at most {} MiB",
                        LIMITS.file_bytes / MIB
                    ));
                }
                bytes += size;
                files.push((entry.path.as_str(), path));
            }
        }
    }
    if files.len() > LIMITS.files {
        problems.push(format!(
            "{} files; a skill may hold at most {}",
            files.len(),
            LIMITS.files
        ));
    }
    if bytes > LIMITS.skill_bytes {
        problems.push(format!(
            "{bytes} bytes in total; a skill may hold at most {} MiB",
            LIMITS.skill_bytes / MIB
        ));
    }
    refuse(origin, &problems)?;

    for (full, path) in files {
        let text = tree.read(full).map_err(io)?;
        if is_lfs_pointer(&text) {
            problems.push(format!(
                "{path}, a Git LFS pointer; skiman does not fetch LFS content"
            ));
        }
        if content == Content::Upstream {
            for (index, line) in text.split(|b| *b == b'\n').enumerate() {
                if CONFLICT_MARKERS
                    .iter()
                    .any(|m| line.starts_with(m.as_bytes()))
                {
                    problems.push(format!(
                        "a skiman conflict-marker line at {path}:{}",
                        index + 1
                    ));
                }
            }
        }
    }
    refuse(origin, &problems)
}

fn is_safe_component(component: &str) -> bool {
    !(component.is_empty()
        || component == "."
        || component == ".."
        || component.eq_ignore_ascii_case(".git"))
}

/// The Git LFS spec's pointer: under 1024 bytes, first line a known
/// `version`, with `oid sha256:<64 hex>` and `size <digits>` lines.
fn is_lfs_pointer(content: &[u8]) -> bool {
    if content.len() >= LFS_POINTER_MAX {
        return false;
    }
    let Ok(text) = std::str::from_utf8(content) else {
        return false;
    };
    let mut lines = text.lines();
    if !lines.next().is_some_and(|l| LFS_VERSIONS.contains(&l)) {
        return false;
    }
    let (mut oid, mut size) = (false, false);
    for line in lines {
        if let Some(hex) = line.strip_prefix("oid sha256:") {
            oid |= hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit());
        } else if let Some(digits) = line.strip_prefix("size ") {
            size |= !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
        }
    }
    oid && size
}

fn refuse(origin: &str, problems: &[String]) -> Result<(), Failure> {
    if problems.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = problems
        .iter()
        .map(|p| format!("{origin} holds {p}"))
        .collect();
    Err(Failure::refused(lines.join("\n")))
}
