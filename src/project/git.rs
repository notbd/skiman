//! What git says about project items: whether the skill repo is in the
//! middle of an operation (read as files, so sync spawns no git), whether a
//! project's own git repo tracks an item (gate 1), and which mirror paths the
//! skill repo ignores.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::git::query_command;

/// What the repo at `root` is in the middle of, as the end of a sentence
/// about it ("is in the middle of a merge", "has a detached HEAD …"), or
/// `None`. Read from the git dir only; a
/// `.git` file is followed to the worktree's git dir.
pub fn operation_in_progress(root: &Path) -> Option<&'static str> {
    let dir = git_dir(root)?;
    let head = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    if !head.starts_with("ref:") {
        return Some("has a detached HEAD (a `git checkout` of a commit, or a bisect)");
    }
    let states = [
        ("MERGE_HEAD", "is in the middle of a merge"),
        ("CHERRY_PICK_HEAD", "is in the middle of a cherry-pick"),
        ("REVERT_HEAD", "is in the middle of a revert"),
        ("BISECT_LOG", "is in the middle of a bisect"),
        ("rebase-merge", "is in the middle of a rebase"),
        ("rebase-apply", "is in the middle of a rebase"),
    ];
    states
        .into_iter()
        .find(|(name, _)| std::fs::symlink_metadata(dir.join(name)).is_ok())
        .map(|(_, phrase)| phrase)
}

/// The git dir of the work tree at `root`: `.git` itself (also when it is a
/// link to a folder), or what a `.git` file points to.
fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let meta = std::fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    Some(root.join(target))
}

/// Whether `dir` is a linked git worktree: its `.git` is a file pointing
/// into `<git dir>/worktrees/`.
pub fn is_linked_worktree(dir: &Path) -> bool {
    let dot_git = dir.join(".git");
    std::fs::symlink_metadata(&dot_git).is_ok_and(|m| m.is_file())
        && std::fs::read_to_string(&dot_git).is_ok_and(|text| {
            text.trim()
                .strip_prefix("gitdir:")
                .is_some_and(|t| t.contains("/worktrees/"))
        })
}

/// For each of `items` (absolute paths of project items on disk): the
/// project git repo that tracks it (`Ok(Some(top))`), `Ok(None)` when none
/// does, or the git error. A folder counts as tracked when any file in it
/// is. One `git rev-parse` per repo found and one `git ls-files` per repo.
pub fn tracking_repos(items: &[PathBuf]) -> BTreeMap<PathBuf, Result<Option<PathBuf>, String>> {
    let mut tops: BTreeMap<PathBuf, Result<PathBuf, String>> = BTreeMap::new();
    let mut by_top: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for item in items {
        // From the item itself: a skill folder may be a git clone of its own.
        let Some(found) = nearest_dot_git(item) else {
            out.insert(item.clone(), Ok(None));
            continue;
        };
        let top = tops
            .entry(found.clone())
            .or_insert_with(|| show_toplevel(&found))
            .clone();
        match top {
            Ok(top) => by_top.entry(top).or_default().push(item.clone()),
            Err(e) => {
                out.insert(item.clone(), Err(e));
            }
        }
    }
    for (top, items) in by_top {
        match tracked_under(&top, &items) {
            Ok(tracked) => {
                for item in items {
                    let is_tracked = tracked.iter().any(|t| t == &item || t.starts_with(&item));
                    out.insert(item, Ok(is_tracked.then(|| top.clone())));
                }
            }
            Err(e) => {
                for item in items {
                    out.insert(item, Err(e.clone()));
                }
            }
        }
    }
    out
}

/// The nearest folder at or above `path` that holds a `.git` entry.
fn nearest_dot_git(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|d| std::fs::symlink_metadata(d.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

fn show_toplevel(dir: &Path) -> Result<PathBuf, String> {
    let output = query_command(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", dir.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git cannot read the repo at {}: {}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let top = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok(top.canonicalize().unwrap_or(top))
}

/// The absolute paths of every file `git ls-files` lists under `items` in the repo at `top`.
fn tracked_under(top: &Path, items: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let rels: Vec<&Path> = items
        .iter()
        .filter_map(|item| item.strip_prefix(top).ok())
        .map(|rel| {
            if rel.as_os_str().is_empty() {
                Path::new(".")
            } else {
                rel
            }
        })
        .collect();
    let output = query_command(top)
        .args(["ls-files", "-z", "--"])
        .args(&rels)
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", top.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed in {}: {}",
            top.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| top.join(String::from_utf8_lossy(p).as_ref()))
        .collect())
}

/// The repo-relative `paths` the skill repo at `root` ignores, each with the
/// rule that ignores it (`<source>:<line>:<pattern>`). Empty when git
/// cannot tell (the repo is not a git repo, git fails).
pub fn ignored(root: &Path, paths: &[String]) -> Vec<(String, String)> {
    if paths.is_empty() {
        return Vec::new();
    }
    // `-z`, which git takes only with `--stdin`, keeps a non-ASCII path as it
    // is (no quotes, no octal escapes): the paths go in NUL-terminated, and
    // each ignored one comes out as four NUL-terminated fields: source, line,
    // pattern, path.
    let Ok(mut child) = query_command(root)
        .args(["check-ignore", "-v", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input: Vec<u8> = paths.iter().flat_map(|p| p.bytes().chain([0])).collect();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let Ok(output) = child.wait_with_output() else {
        return Vec::new();
    };
    let _ = writer.join();
    let text = String::from_utf8_lossy(&output.stdout);
    let fields: Vec<&str> = text.split_terminator('\0').collect();
    fields
        .as_chunks::<4>()
        .0
        .iter()
        .map(|[source, line, pattern, path]| {
            ((*path).to_owned(), format!("{source}:{line}:{pattern}"))
        })
        .collect()
}

/// How many paths under `projects/` `git status` lists as changed or new;
/// `None` when git cannot tell.
pub fn uncommitted_mirror_changes(root: &Path) -> Option<usize> {
    let output = query_command(root)
        .args([
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            "projects",
        ])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).lines().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dot_git_link_to_a_folder_still_reports_the_operation_in_progress() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let git_dir = root.join("elsewhere.git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git_dir.join("MERGE_HEAD"), "0123456789abcdef\n").unwrap();
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::os::unix::fs::symlink(&git_dir, repo.join(".git")).unwrap();

        assert_eq!(
            operation_in_progress(&repo),
            Some("is in the middle of a merge")
        );
    }
}
