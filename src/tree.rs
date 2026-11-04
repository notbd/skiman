//! A read-only view of a folder's content that the rules and the tree hash
//! work on, so the same checks run on the working tree, on content a write
//! command is about to write ([`Overlay`]) and on other snapshots.
//!
//! Paths are relative to the view's root, `/`-separated, without a leading
//! `/`; `""` is the root itself.

use std::collections::BTreeSet;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// File names ignored by hashing, diffing and the rules (spec Data formats, Tree hash).
pub const IGNORED_NAMES: &[&str] = &[".DS_Store", "Thumbs.db"];

/// Whether the last component of `path` is `.DS_Store` or `Thumbs.db`.
pub fn is_ignored(path: &str) -> bool {
    IGNORED_NAMES.contains(&basename(path))
}

pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `parent/name`, or `name` when `parent` is the root.
pub fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

/// What kind of entry a path is, never following a symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A regular file; `executable` is the owner's execute bit.
    File {
        size: u64,
        executable: bool,
    },
    Dir,
    Symlink,
    /// FIFO, socket or device: never read.
    Special,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the view's root, not to the folder that was walked.
    pub path: String,
    pub kind: Kind,
}

pub trait Tree {
    /// The kind of the entry at `path`; `None` when nothing is there.
    fn kind(&self, path: &str) -> io::Result<Option<Kind>>;
    /// Every entry below `dir` at any depth (not `dir` itself), sorted by path
    /// bytes, without descending into symlinks. Empty when `dir` is not a folder.
    fn walk(&self, dir: &str) -> io::Result<Vec<Entry>>;
    /// The content of the regular file at `path`.
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;

    /// Names of the entries directly in `dir`, sorted.
    fn children(&self, dir: &str) -> io::Result<Vec<Entry>> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        Ok(self
            .walk(dir)?
            .into_iter()
            .filter(|e| !e.path[prefix.len()..].contains('/'))
            .collect())
    }
}

/// Files on disk below `root`.
#[derive(Debug, Clone)]
pub struct WorkingTree {
    root: PathBuf,
}

impl WorkingTree {
    pub fn new(root: impl Into<PathBuf>) -> WorkingTree {
        WorkingTree { root: root.into() }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        }
    }
}

fn kind_of(meta: &std::fs::Metadata) -> Kind {
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        Kind::Symlink
    } else if file_type.is_dir() {
        Kind::Dir
    } else if file_type.is_file() {
        Kind::File {
            size: meta.size(),
            executable: meta.permissions().mode() & 0o100 != 0,
        }
    } else {
        Kind::Special
    }
}

impl Tree for WorkingTree {
    fn kind(&self, path: &str) -> io::Result<Option<Kind>> {
        match std::fs::symlink_metadata(self.path(path)) {
            Ok(meta) => Ok(Some(kind_of(&meta))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn walk(&self, dir: &str) -> io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        if self.kind(dir)? == Some(Kind::Dir) {
            walk_dir(&self.root, dir, &mut out)?;
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.path(path))
    }
}

/// Names that are not valid UTF-8 are converted lossily; path safety at fetch
/// time keeps them out of the skill repo.
fn walk_dir(root: &Path, dir: &str, out: &mut Vec<Entry>) -> io::Result<()> {
    for entry in std::fs::read_dir(root.join(dir))? {
        let entry = entry?;
        let path = join(dir, &entry.file_name().to_string_lossy());
        let kind = kind_of(&std::fs::symlink_metadata(entry.path())?);
        if kind == Kind::Dir {
            walk_dir(root, &path, out)?;
        }
        out.push(Entry { path, kind });
    }
    Ok(())
}

/// A tree with regular files laid over it (non-executable unless named in
/// [`Overlay::with_executable`]): what the tree will look like once a write
/// command has written them. Folders above an added file appear as folders.
pub struct Overlay<'a> {
    base: &'a dyn Tree,
    files: Vec<(String, Vec<u8>)>,
    executable: BTreeSet<String>,
    /// Folders of `base` whose content is hidden: only added files show there.
    replaced: Vec<String>,
}

impl<'a> Overlay<'a> {
    pub fn new(base: &'a dyn Tree, files: Vec<(String, Vec<u8>)>) -> Overlay<'a> {
        Overlay {
            base,
            files,
            executable: BTreeSet::new(),
            replaced: Vec::new(),
        }
    }

    /// Hides everything `base` has at and below each of `dirs`, so those
    /// folders hold exactly the added files below them (a folder written as
    /// a whole, such as a merged skill replacing `skills/<name>`).
    pub fn replacing(mut self, dirs: Vec<String>) -> Overlay<'a> {
        self.replaced = dirs;
        self
    }

    fn is_replaced(&self, path: &str) -> bool {
        self.replaced.iter().any(|d| {
            path == d
                || path
                    .strip_prefix(d.as_str())
                    .is_some_and(|r| r.starts_with('/'))
        })
    }

    /// Marks the added files at `paths` as executable.
    pub fn with_executable(mut self, paths: BTreeSet<String>) -> Overlay<'a> {
        self.executable = paths;
        self
    }

    fn file(&self, path: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, c)| c.as_slice())
    }

    fn is_added_dir(&self, path: &str) -> bool {
        path.is_empty()
            || self
                .files
                .iter()
                .any(|(p, _)| p.strip_prefix(path).is_some_and(|r| r.starts_with('/')))
    }
}

impl Tree for Overlay<'_> {
    fn kind(&self, path: &str) -> io::Result<Option<Kind>> {
        if let Some(content) = self.file(path) {
            return Ok(Some(Kind::File {
                size: content.len() as u64,
                executable: self.executable.contains(path),
            }));
        }
        if self.is_added_dir(path) {
            return Ok(Some(Kind::Dir));
        }
        if self.is_replaced(path) {
            return Ok(None);
        }
        self.base.kind(path)
    }

    fn walk(&self, dir: &str) -> io::Result<Vec<Entry>> {
        let mut out = self.base.walk(dir)?;
        out.retain(|e| !self.is_replaced(&e.path));
        let inside =
            |p: &str| dir.is_empty() || p.strip_prefix(dir).is_some_and(|r| r.starts_with('/'));
        for (path, _) in &self.files {
            if !inside(path) {
                continue;
            }
            let mut ancestor = path.as_str();
            while let Some((parent, _)) = ancestor.rsplit_once('/') {
                if !inside(parent) {
                    break;
                }
                out.push(Entry {
                    path: parent.to_owned(),
                    kind: Kind::Dir,
                });
                ancestor = parent;
            }
            out.push(Entry {
                path: path.clone(),
                kind: self.kind(path)?.expect("added file exists"),
            });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        // An added path replaces the base entry at the same path; keep the last one pushed.
        out.reverse();
        out.dedup_by(|a, b| a.path == b.path);
        out.reverse();
        Ok(out)
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        match self.file(path) {
            Some(content) => Ok(content.to_vec()),
            None if self.is_replaced(path) => Err(io::ErrorKind::NotFound.into()),
            None => self.base.read(path),
        }
    }
}

/// The spec's **tree hash** of the folder `dir` in `tree`, as `sha256:<hex>`:
/// sha256 over the regular files sorted by UTF-8 path bytes, each contributing
/// `path \0 executable-bit \0 sha256(content) \n` with `path` relative to
/// `dir`. Symlinks, special files, `.DS_Store`, `Thumbs.db` and empty folders
/// do not take part. A missing folder hashes like an empty one.
pub fn tree_hash(tree: &dyn Tree, dir: &str) -> io::Result<String> {
    let skip = if dir.is_empty() { 0 } else { dir.len() + 1 };
    let mut hasher = Sha256::new();
    for entry in tree.walk(dir)? {
        let Kind::File { executable, .. } = entry.kind else {
            continue;
        };
        if is_ignored(&entry.path) {
            continue;
        }
        let content = Sha256::digest(tree.read(&entry.path)?);
        hash_file(&mut hasher, &entry.path[skip..], executable, &content);
    }
    Ok(format!("sha256:{}", hex(&hasher.finalize())))
}

/// [`tree_hash`] of files held in memory: `(path relative to the folder,
/// executable, content)`, in any order. `.DS_Store` and `Thumbs.db` are skipped.
pub fn files_hash<'a>(files: impl IntoIterator<Item = (&'a str, bool, &'a [u8])>) -> String {
    let mut files: Vec<_> = files
        .into_iter()
        .filter(|(path, _, _)| !is_ignored(path))
        .collect();
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut hasher = Sha256::new();
    for (path, executable, content) in files {
        hash_file(&mut hasher, path, executable, &Sha256::digest(content));
    }
    format!("sha256:{}", hex(&hasher.finalize()))
}

/// One file's line of the tree hash: `path \0 executable-bit \0 sha256(content) \n`.
fn hash_file(hasher: &mut Sha256, path: &str, executable: bool, content_sha: &[u8]) {
    hasher.update(path.as_bytes());
    hasher.update(if executable {
        b"\x001\x00"
    } else {
        b"\x000\x00"
    });
    hasher.update(hex(content_sha).as_bytes());
    hasher.update(b"\n");
}

/// [`tree_hash`] of a folder on disk.
pub fn tree_hash_of_dir(dir: &Path) -> io::Result<String> {
    tree_hash(&WorkingTree::new(dir), "")
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
