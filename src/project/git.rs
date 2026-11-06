//! What git says about project items: whether the skill repo is in the
//! middle of an operation (read as files, so sync spawns no git), whether a
//! project's own git repo tracks an item (gate 1), and which mirror paths the
//! skill repo ignores.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::git::query_command;

/// The git operation the repo at `root` is in the middle of, as a phrase
/// ("a merge", "a detached HEAD"), or `None`. Read from the git dir only; a
/// `.git` file is followed to the worktree's git dir.
pub fn operation_in_progress(root: &Path) -> Option<&'static str> {
    let dir = git_dir(root)?;
    let head = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    if !head.starts_with("ref:") {
        return Some("a detached HEAD (a `git checkout` of a commit, or a bisect)");
    }
    let states = [
        ("MERGE_HEAD", "a merge"),
        ("CHERRY_PICK_HEAD", "a cherry-pick"),
        ("REVERT_HEAD", "a revert"),
        ("BISECT_LOG", "a bisect"),
        ("rebase-merge", "a rebase"),
        ("rebase-apply", "a rebase"),
    ];
    states
        .into_iter()
        .find(|(name, _)| std::fs::symlink_metadata(dir.join(name)).is_ok())
        .map(|(_, phrase)| phrase)
}

/// The git dir of the work tree at `root`: `.git` itself, or what a `.git`
/// file points to.
fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let meta = std::fs::symlink_metadata(&dot_git).ok()?;
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
        let Some(found) = item.parent().and_then(nearest_dot_git) else {
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

/// The nearest folder at or above `dir` that holds a `.git` entry.
fn nearest_dot_git(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
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
    let Ok(output) = query_command(root)
        .args(["check-ignore", "-v", "--"])
        .args(paths)
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (rule, path) = line.split_once('\t')?;
            Some((path.to_owned(), rule.to_owned()))
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
