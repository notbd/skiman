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

/// How a cache folder's `CACHEDIR.TAG` starts (<https://bford.info/cachedir/>).
const CACHE_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

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

/// Walks `root` from its root-relative folder `from` (`""`: all of it).
/// `skip` holds realpaths never entered ([`never_entered`]); an item that
/// overlaps one of `linked` goes to [`Discovery::skipped`]. Each folder on
/// the way to `from` is judged as the walk judges it, under its name on
/// disk: one the walk would not enter, an item, or another spelling of a
/// folder's name holds nothing to find.
pub fn discover(root: &Root, skip: &[PathBuf], linked: &Linked, from: &str) -> Discovery {
    let mut found = Discovery::default();
    let (mut dir, mut rel) = (root.dir.clone(), String::new());
    for name in from.split('/').filter(|n| !n.is_empty()) {
        let child = below(&rel, name);
        let path = dir.join(name);
        // A name the volume folds onto another (`WEB` for `web`) is not one
        // the walk meets: its items would be recorded under that spelling.
        if !matches!(judge(root, &path, name, &child, skip), Entry::Enter)
            || path.canonicalize().ok().as_ref() != Some(&path)
        {
            return found;
        }
        (dir, rel) = (path, child);
    }
    walk(root, &dir, &rel, skip, &mut found);
    found.items.sort();
    for item in std::mem::take(&mut found.items) {
        match linked.item_overlap(&item.disk(root)) {
            Some(why) => found.skipped.push((item, why)),
            None => found.items.push(item),
        }
    }
    found
}

fn below(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_owned()
    } else {
        format!("{rel}/{name}")
    }
}

/// What the walk does with one entry.
enum Entry {
    Pass,
    Item(Item),
    /// A project-side staging folder left by an earlier write.
    Leftover,
    Enter,
}

/// Judges the entry `name` at `path`, root-relative `child`.
fn judge(root: &Root, path: &Path, name: &str, child: &str, skip: &[PathBuf]) -> Entry {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Entry::Pass;
    };
    if is_excluded(child, &root.exclude) || meta.is_symlink() {
        return Entry::Pass;
    }
    if meta.is_file() {
        return Item::at(root, child, false).map_or(Entry::Pass, Entry::Item);
    }
    if !meta.is_dir() || SKIPPED_NAMES.contains(&name) || skip.iter().any(|s| s == path) {
        return Entry::Pass;
    }
    if name.starts_with(PROJECT_PREFIX) {
        return Entry::Leftover;
    }
    if is_linked_worktree(path) {
        return Entry::Pass;
    }
    match Item::at(root, child, true) {
        // A skill folder's location: nothing inside it is an item of its own.
        Some(item) if item.kind == Kind::Skill => {
            if path.join("SKILL.md").is_file() {
                Entry::Item(item)
            } else {
                Entry::Pass
            }
        }
        _ if is_cache(path) => Entry::Pass,
        _ => Entry::Enter,
    }
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
        let Some(name) = name.to_str() else {
            continue;
        };
        let (path, child) = (dir.join(name), below(rel, name));
        match judge(root, &path, name, &child, skip) {
            Entry::Pass => {}
            Entry::Item(item) => found.items.push(item),
            Entry::Leftover => found.swap_leftovers.push(path),
            Entry::Enter => walk(root, &path, &child, skip, found),
        }
    }
}

/// Whether `dir` is tagged as a cache (build output, such as Rust's
/// `target/`): its `CACHEDIR.TAG` starts with the signature.
fn is_cache(dir: &Path) -> bool {
    let mut head = [0; CACHE_SIGNATURE.len()];
    std::fs::File::open(dir.join("CACHEDIR.TAG"))
        .and_then(|mut tag| std::io::Read::read_exact(&mut tag, &mut head))
        .is_ok_and(|()| head == CACHE_SIGNATURE)
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
