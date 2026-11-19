//! Discovery (project-scope.md "Discovery"): every project item candidate under
//! a root, found with `read_dir` and `lstat`, never following a symlink. Used
//! by `project adopt` and `doctor`; sync never walks the roots.

use std::path::{Path, PathBuf};

use super::git::is_linked_worktree;
use super::item::Item;
use super::pattern::Kind;
use super::root::Root;
use crate::canon::Canon;
use crate::linked::Linked;
use crate::paths::Paths;
use crate::staging::PROJECT_PREFIX;

/// Folder names discovery never enters, at any depth.
const SKIPPED_NAMES: &[&str] = &[".git", "node_modules"];

#[derive(Debug, Default)]
pub struct Discovery {
    /// Items found on disk: regular files and real skill folders, in path order.
    pub items: Vec<Item>,
    /// Items found that overlap a linked folder (spec "Project items"), with
    /// why: they are never adopted.
    pub skipped: Vec<(Item, String)>,
    /// Project-side staging folders left by an earlier write.
    pub swap_leftovers: Vec<PathBuf>,
    /// Folders that could not be read, with the error.
    pub errors: Vec<String>,
}

/// Whether the root-relative `rel` is excluded by one of `exclude`: an entry
/// without `/` names a file or folder at any depth, an entry with `/` a
/// root-relative path and everything under it.
pub fn is_excluded(rel: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|entry| {
        let entry = entry.trim_matches('/');
        if entry.contains('/') {
            rel == entry || rel.starts_with(&format!("{entry}/"))
        } else {
            rel.split('/').any(|part| part == entry)
        }
    })
}

/// The realpaths no project command works in, though a root may hold them:
/// the canon and skiman's state folder. Discovery never enters them,
/// and `skiman project` refuses a path given inside them.
pub fn never_entered(paths: &Paths, repo: &Canon) -> [PathBuf; 2] {
    [
        repo.root().to_path_buf(),
        paths
            .state_dir()
            .canonicalize()
            .unwrap_or(paths.state_dir()),
    ]
}

/// Walks `root`. `skip` holds realpaths never entered ([`never_entered`]);
/// an item that overlaps one of `linked` goes to [`Discovery::skipped`].
pub fn discover(root: &Root, skip: &[PathBuf], linked: &Linked) -> Discovery {
    let mut found = Discovery::default();
    walk(root, &root.dir, "", skip, &mut found);
    found.items.sort();
    for item in std::mem::take(&mut found.items) {
        match linked.item_overlap(&item.disk(root)) {
            Some(why) => found.skipped.push((item, why)),
            None => found.items.push(item),
        }
    }
    found
}

fn walk(root: &Root, dir: &Path, rel: &str, skip: &[PathBuf], found: &mut Discovery) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            found
                .errors
                .push(format!("cannot read {}: {e}", dir.display()));
            return;
        }
    };
    let mut names: Vec<_> = entries.flatten().map(|e| e.file_name()).collect();
    names.sort();
    for name in names {
        let Some(name) = name.to_str().map(str::to_owned) else {
            continue;
        };
        let path = dir.join(&name);
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if is_excluded(&child, &root.exclude) || meta.is_symlink() {
            continue;
        }
        if meta.is_file() {
            if let Some(item) = Item::at(root, &child, false) {
                found.items.push(item);
            }
            continue;
        }
        if !meta.is_dir() || SKIPPED_NAMES.contains(&name.as_str()) || skip.contains(&path) {
            continue;
        }
        if name.starts_with(PROJECT_PREFIX) {
            found.swap_leftovers.push(path);
            continue;
        }
        if is_linked_worktree(&path) {
            continue;
        }
        match Item::at(root, &child, true) {
            // A skill folder's location: nothing inside it is an item of its own.
            Some(item) if item.kind == Kind::Skill => {
                if path.join("SKILL.md").is_file() {
                    found.items.push(item);
                }
            }
            _ => walk(root, &path, &child, skip, found),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_excluded;

    #[test]
    fn exclude_by_name_and_by_path() {
        let exclude = vec!["_archive".to_owned(), "a/b".to_owned()];
        assert!(is_excluded("x/_archive/y/AGENTS.md", &exclude));
        assert!(is_excluded("a/b/AGENTS.md", &exclude));
        assert!(is_excluded("a/b", &exclude));
        assert!(!is_excluded("a/bc/AGENTS.md", &exclude));
        assert!(!is_excluded("x/archive/AGENTS.md", &exclude));
    }
}
