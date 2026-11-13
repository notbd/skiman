//! Reading a third-party skill's upstream (spec Source fetching): a commit
//! a ref names, fetched into the cache, or a local folder as it is now; then
//! the files of one folder of it. `add`, `update` and `import` read every
//! upstream through here, before they take the repo lock, as fetching may
//! use the network and ask for credentials.

use std::path::{Path, PathBuf};

use crate::discovery;
use crate::failure::Failure;
use crate::fetch::{Cache, RefKind, ResolvedRef};
use crate::lockfile::LockEntry;
use crate::paths::Paths;
use crate::snapshot::{self, Snapshot};
use crate::source::{Source, SourceKind};
use crate::tree;

/// An upstream at one version, ready to read.
pub enum Upstream {
    /// A commit in the cache.
    Git {
        cache: Cache,
        commit: String,
        /// The ref it was read at; `None` when following the default branch.
        resolved: Option<ResolvedRef>,
        /// The default branch, when following it.
        default_branch: Option<String>,
    },
    /// A local folder: the root a source's `path` is below.
    Local { dir: PathBuf },
}

impl Upstream {
    /// `cache` at the version `git_ref` names (a full sha, else a branch,
    /// then a tag), or without one at the head of its default branch.
    pub fn at_ref(cache: Cache, git_ref: Option<&str>) -> Result<Upstream, Failure> {
        match git_ref {
            Some(name) => {
                let resolved = cache.resolve_ref(name)?;
                Upstream::at(cache, resolved.commit.clone(), Some(resolved), None)
            }
            None => {
                let head = cache.default_branch()?;
                Upstream::at(cache, head.commit, None, Some(head.branch))
            }
        }
    }

    /// `commit` of `cache`, fetched when the cache does not hold it yet;
    /// `resolved` and `default_branch` say how it was chosen.
    pub fn at(
        cache: Cache,
        commit: String,
        resolved: Option<ResolvedRef>,
        default_branch: Option<String>,
    ) -> Result<Upstream, Failure> {
        cache.fetch_commit(&commit)?;
        Ok(Upstream::Git {
            cache,
            commit,
            resolved,
            default_branch,
        })
    }

    /// Every file below `path`: its path relative to `path`, what
    /// [`Upstream::read_files`] reads it by (its blob id in a git upstream,
    /// so no name can break `cat-file`'s line protocol, or its path from the
    /// upstream's root in a local folder), and its size.
    pub fn list(&self, path: &str) -> Result<Vec<(String, String, u64)>, Failure> {
        match self {
            Upstream::Git { cache, commit, .. } => snapshot::list(cache, commit, path),
            Upstream::Local { dir } => Ok(snapshot::list_local(&local_folder(dir, path))?
                .into_iter()
                .map(|(file, size)| (file.clone(), discovery::join(path, &file), size))
                .collect()),
        }
    }

    /// The contents of the files `keys` name, as [`Upstream::list`] gives
    /// them. In a local folder, anything but a regular file reads as empty: a
    /// symlink is not followed and a FIFO is not opened.
    pub fn read_files(&self, keys: &[&str]) -> Result<Vec<Vec<u8>>, Failure> {
        match self {
            Upstream::Git { cache, .. } => snapshot::cat_blobs(cache.git(), keys.iter().copied()),
            Upstream::Local { dir } => keys
                .iter()
                .map(|p| {
                    let file = join(dir, p);
                    let io = |e: std::io::Error| {
                        Failure::operational(format!("cannot read {}: {e}", file.display()))
                    };
                    if !std::fs::symlink_metadata(&file).map_err(io)?.is_file() {
                        return Ok(Vec::new());
                    }
                    std::fs::read(&file).map_err(io)
                })
                .collect(),
        }
    }

    /// The skill folder `path` (`""` = the root) as a third-party upstream:
    /// without its git files.
    pub fn snapshot(&self, path: &str) -> Result<Snapshot, Failure> {
        match self {
            Upstream::Git { cache, commit, .. } => Snapshot::read(cache, commit, path),
            Upstream::Local { dir } => Snapshot::read_local_upstream(&local_folder(dir, path)),
        }
    }

    pub fn resolved(&self) -> Option<&ResolvedRef> {
        match self {
            Upstream::Git { resolved, .. } => resolved.as_ref(),
            Upstream::Local { .. } => None,
        }
    }

    /// The lock entry of a skill whose snapshot `snapshot` was read here.
    pub fn lock_entry(&self, snapshot: &Snapshot) -> LockEntry {
        let commit = match self {
            Upstream::Git { commit, .. } => Some(commit.clone()),
            Upstream::Local { .. } => None,
        };
        let git_ref = self.resolved().map(|r| r.name.clone());
        let snapshot = snapshot.tree_hash();
        LockEntry {
            commit,
            git_ref,
            snapshot,
        }
    }

    /// `at <commit> (default branch main)`, `at <commit> (tag v1)`, `(local folder)`.
    pub fn version(&self) -> String {
        match self {
            Upstream::Git {
                commit,
                resolved,
                default_branch,
                ..
            } => match (resolved, default_branch) {
                (Some(r), _) if r.kind == RefKind::Commit => format!("at {commit} (commit)"),
                (Some(r), _) => format!("at {commit} ({} {})", r.kind.as_str(), r.name),
                (None, Some(branch)) => format!("at {commit} (default branch {branch})"),
                (None, None) => format!("at {commit}"),
            },
            Upstream::Local { .. } => "(local folder)".to_owned(),
        }
    }
}

/// The local folder a recorded `source.url` (`/` or `~`) names, as `add`
/// made it absolute ([`Source::parse`]: `~` expanded, `.` and `..` taken by
/// the letter, not through links).
pub fn local_root(paths: &Paths, url: &str) -> Result<PathBuf, Failure> {
    let SourceKind::Local { dir } = Source::parse(url, paths.home(), Path::new("/"))?.kind else {
        unreachable!("a `/` or `~` source is a local folder");
    };
    Ok(dir)
}

/// `dir/path` as it is read: a link there is followed, so a source that is
/// a link reads the folder it points to. Links below it never are.
fn local_folder(dir: &Path, path: &str) -> PathBuf {
    let folder = join(dir, path);
    match folder.canonicalize() {
        Ok(target) if folder.is_symlink() => target,
        _ => folder,
    }
}

/// `dir/rel`, or `dir` for `""`.
pub fn join(dir: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        dir.to_path_buf()
    } else {
        dir.join(rel)
    }
}

/// The snapshot's submodules and git files, which are not installed, as
/// `add`, `update` and `import` list them, each line after `indent`.
pub fn print_not_installed(snapshot: &Snapshot, indent: &str) {
    for submodule in &snapshot.submodules {
        out!("{indent}{submodule}  [submodule, not fetched]");
    }
    for path in &snapshot.git_files {
        out!("{indent}{path}  [not installed]");
    }
}

/// `  [script, executable]` after a path in a file list, flagging what
/// agents may run; empty for anything else.
pub fn run_flags(path: &str, content: &[u8], executable: bool) -> String {
    let script = is_script(path, content).then_some("script");
    let flags: Vec<&str> = script
        .into_iter()
        .chain(executable.then_some("executable"))
        .collect();
    if flags.is_empty() {
        String::new()
    } else {
        format!("  [{}]", flags.join(", "))
    }
}

/// A file agents may run: it starts with `#!` or has a script extension.
fn is_script(path: &str, content: &[u8]) -> bool {
    const EXTENSIONS: &str =
        "sh bash zsh fish py rb pl php js mjs cjs ts ps1 bat cmd applescript scpt";
    content.starts_with(b"#!")
        || tree::basename(path)
            .rsplit_once('.')
            .is_some_and(|(_, ext)| EXTENSIONS.split(' ').any(|e| e.eq_ignore_ascii_case(ext)))
}
