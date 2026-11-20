//! Reading a folder of an upstream commit from the git cache by objects
//! (`ls-tree`, `cat-file`), never by checkout (spec Source fetching), so no
//! filter, hook or attribute of the upstream can run.
//!
//! A third-party upstream's `.gitignore`, `.gitattributes` and `.gitmodules`
//! configure the upstream's git; in the canon they would configure its
//! git instead (hide skill files from commits, filter them). Reading such an
//! upstream ([`Snapshot::read`], [`Snapshot::read_local_upstream`]) leaves
//! them out of the files and lists them in [`Snapshot::git_files`].

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::failure::Failure;
use crate::fetch::Cache;
use crate::git;
use crate::path_safety::{self, Content};
use crate::rules::LIMITS;
use crate::tree::{self, Kind, Tree, WorkingTree, is_ignored};

/// One regular file of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFile {
    /// Relative to the snapshot folder, `/`-separated.
    pub path: String,
    pub executable: bool,
    pub content: Vec<u8>,
}

/// The files of one folder of an upstream commit, sorted by path bytes.
/// Ignored names ([`tree::IGNORED_NAMES`]) are left out (never copied into the
/// canon).
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub files: Vec<SnapshotFile>,
    /// Submodules (gitlinks): not fetched, listed in the summary.
    pub submodules: Vec<String>,
    /// The upstream's files that configure git: not installed, listed in the
    /// summary.
    pub git_files: Vec<String>,
}

/// Names of the files that configure a git repository, matched in any ASCII
/// case at any depth, like `.git`.
const GIT_FILES: &[&str] = &[".gitignore", ".gitattributes", ".gitmodules"];

impl Snapshot {
    pub fn file(&self, path: &str) -> Option<&SnapshotFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// The spec's tree hash of these files (`sha256:<hex>`).
    pub fn tree_hash(&self) -> String {
        tree::files_hash(
            self.files
                .iter()
                .map(|f| (f.path.as_str(), f.executable, f.content.as_slice())),
        )
    }

    /// The regular files of the folder `dir` on disk, read once (a missing
    /// folder has none). Ignored names are left out; a symlink or a special
    /// file is refused (exit 1).
    pub fn from_folder(dir: &Path) -> Result<Snapshot, Failure> {
        let io =
            |e: std::io::Error| Failure::operational(format!("cannot read {}: {e}", dir.display()));
        let working_tree = WorkingTree::new(dir);
        let mut snapshot = Snapshot::default();
        for entry in working_tree.walk("").map_err(io)? {
            let path = dir.join(&entry.path);
            match entry.kind {
                Kind::Dir => {}
                _ if is_ignored(&entry.path) => {}
                Kind::File { executable, .. } => {
                    snapshot.files.push(SnapshotFile {
                        content: working_tree.read(&entry.path).map_err(io)?,
                        path: entry.path,
                        executable,
                    });
                }
                Kind::Symlink | Kind::Special => {
                    return Err(Failure::refused(format!(
                        "{} is not a regular file or folder (symlinks and special files are not allowed in skills); remove it",
                        path.display()
                    )));
                }
            }
        }
        snapshot.files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(snapshot)
    }

    /// Reads folder `path` (`""` = the root) of `commit`, which must be in
    /// the cache, as a third-party upstream: without its git files.
    pub fn read(cache: &Cache, commit: &str, path: &str) -> Result<Snapshot, Failure> {
        let origin = if path.is_empty() {
            cache.url().to_owned()
        } else {
            format!("{} ({path})", cache.url())
        };
        let treeish = folder(cache, commit, path)?;
        Snapshot::read_tree(|| cache.git(), &treeish, &origin, Content::Upstream)
            .map(Snapshot::without_git_files)
    }

    /// Reads the local folder `dir` as a third-party upstream:
    /// [`Snapshot::read_local`] without its git files.
    pub fn read_local_upstream(dir: &Path) -> Result<Snapshot, Failure> {
        Snapshot::read_local(dir).map(Snapshot::without_git_files)
    }

    /// Moves the files named in [`GIT_FILES`] from `files` to `git_files`.
    fn without_git_files(mut self) -> Snapshot {
        let (git_files, files) = self.files.into_iter().partition(|file: &SnapshotFile| {
            let name = tree::basename(&file.path);
            GIT_FILES.iter().any(|g| name.eq_ignore_ascii_case(g))
        });
        self.files = files;
        self.git_files = git_files.into_iter().map(|file| file.path).collect();
        self
    }

    /// Reads `treeish` from the repository `git` commands run on; `origin`
    /// names where it comes from in messages. The listing passes
    /// [`path_safety::check`] as `content` before any file content is read.
    pub fn read_tree(
        git: impl Fn() -> Command,
        treeish: &str,
        origin: &str,
        content: Content,
    ) -> Result<Snapshot, Failure> {
        let mut ls = git();
        ls.args(["ls-tree", "-r", "-t", "-z", "-l", "--full-tree", treeish]);
        let output = git::run(ls, &format!("listing {treeish} of {origin}"))?;

        let mut snapshot = Snapshot::default();
        let mut listing = Listing {
            git: &git,
            entries: Vec::new(),
            oids: BTreeMap::new(),
            contents: RefCell::new(None),
        };
        for record in output.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let entry = Entry::parse(record, origin)?;
            let kind = match entry.mode.as_str() {
                "040000" => Kind::Dir,
                "100644" | "100755" => Kind::File {
                    size: entry.size.ok_or_else(|| entry.unexpected())?,
                    executable: entry.mode == "100755",
                },
                "120000" => Kind::Symlink,
                "160000" => {
                    snapshot.submodules.push(entry.path);
                    continue;
                }
                mode => {
                    return Err(Failure::refused(format!(
                        "{origin} holds {} with unsupported git mode {mode}",
                        entry.path
                    )));
                }
            };
            if matches!(kind, Kind::File { .. }) && !is_ignored(&entry.path) {
                listing.oids.insert(entry.path.clone(), entry.oid);
            }
            listing.entries.push(tree::Entry {
                path: entry.path,
                kind,
            });
        }
        listing.entries.sort_by(|a, b| a.path.cmp(&b.path));
        path_safety::check(&listing, "", origin, content)?;

        let io = |e: std::io::Error| Failure::operational(e.to_string());
        for entry in &listing.entries {
            if let Kind::File { executable, .. } = entry.kind
                && !is_ignored(&entry.path)
            {
                snapshot.files.push(SnapshotFile {
                    path: entry.path.clone(),
                    executable,
                    content: listing.read(&entry.path).map_err(io)?,
                });
            }
        }
        Ok(snapshot)
    }
}

/// A git listing seen as a [`Tree`]: the regular files are read in one
/// `cat-file --batch` on the first [`Tree::read`].
struct Listing<'a> {
    git: &'a dyn Fn() -> Command,
    /// Sorted by path bytes.
    entries: Vec<tree::Entry>,
    /// Path to blob id of every regular file that is not ignored.
    oids: BTreeMap<String, String>,
    contents: RefCell<Option<BTreeMap<String, Vec<u8>>>>,
}

impl Tree for Listing<'_> {
    fn kind(&self, path: &str) -> std::io::Result<Option<Kind>> {
        Ok(self.entries.iter().find(|e| e.path == path).map(|e| e.kind))
    }

    fn walk(&self, dir: &str) -> std::io::Result<Vec<tree::Entry>> {
        Ok(below(&self.entries, dir))
    }

    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        let mut contents = self.contents.borrow_mut();
        if contents.is_none() {
            let blobs = cat_blobs((self.git)(), self.oids.values().map(String::as_str))
                .map_err(|f| std::io::Error::other(f.message))?;
            *contents = Some(self.oids.keys().cloned().zip(blobs).collect());
        }
        contents
            .as_ref()
            .and_then(|c| c.get(path).cloned())
            .ok_or_else(|| std::io::Error::other(format!("{path} is not a regular file")))
    }
}

/// The path, blob id and size of every file and symlink below folder `path`
/// of `commit` (relative to it, names that are not UTF-8 converted lossily),
/// without reading any content: what skill discovery looks at. A `path` that
/// is not a folder at `commit` is refused (exit 1).
pub fn list(
    cache: &Cache,
    commit: &str,
    path: &str,
) -> Result<Vec<(String, String, u64)>, Failure> {
    let treeish = folder(cache, commit, path)?;
    let mut ls = cache.git();
    ls.args(["ls-tree", "-r", "-z", "-l", "--full-tree", &treeish]);
    let listing = git::run(ls, &format!("listing {treeish} of {}", cache.url()))?;
    Ok(listing
        .split(|b| *b == 0)
        .filter(|record| !record.starts_with(b"160000 "))
        .filter_map(|record| {
            let tab = record.iter().position(|b| *b == b'\t')?;
            // `<mode> SP <type> SP <oid> SP+ <size> TAB <path>`
            let head = std::str::from_utf8(&record[..tab]).ok()?;
            let [_, _, oid, size] = head.split_whitespace().collect::<Vec<_>>()[..] else {
                return None;
            };
            let path = String::from_utf8_lossy(&record[tab + 1..]).into_owned();
            Some((path, oid.to_owned(), size.parse().ok()?))
        })
        .collect())
}

/// The path and size (0 for anything but a regular file) of every
/// non-folder entry below the local folder `dir`, never looking inside
/// `.git`. A missing `dir` is refused (exit 1).
pub fn list_local(dir: &Path) -> Result<Vec<(String, u64)>, Failure> {
    require_folder(dir)?;
    let folder = LocalFolder::new(dir, false)
        .map_err(|e| Failure::operational(format!("cannot read {}: {e}", dir.display())))?;
    Ok(folder
        .entries
        .into_iter()
        .filter_map(|e| match e.kind {
            Kind::Dir => None,
            Kind::File { size, .. } => Some((e.path, size)),
            _ => Some((e.path, 0)),
        })
        .collect())
}

/// Refuses (exit 1) a `dir` that is not a folder.
fn require_folder(dir: &Path) -> Result<(), Failure> {
    if dir.is_dir() {
        return Ok(());
    }
    Err(Failure::refused(format!(
        "the local folder {} does not exist",
        dir.display()
    )))
}

impl Snapshot {
    /// Reads the local folder `dir` as it is now, without `.git`, after
    /// [`path_safety::check`] passed on it.
    pub fn read_local(dir: &Path) -> Result<Snapshot, Failure> {
        let label = dir.display().to_string();
        let io = |e: std::io::Error| Failure::operational(format!("cannot read {label}: {e}"));
        require_folder(dir)?;
        let folder = LocalFolder::new(dir, true).map_err(io)?;
        path_safety::check(&folder, "", &label, Content::Upstream)?;
        let mut snapshot = Snapshot::default();
        for entry in &folder.entries {
            if let Kind::File { executable, .. } = entry.kind
                && !is_ignored(&entry.path)
            {
                snapshot.files.push(SnapshotFile {
                    content: folder.read(&entry.path).map_err(io)?,
                    path: entry.path.clone(),
                    executable,
                });
            }
        }
        Ok(snapshot)
    }
}

/// The entries of a listing that lie below folder `dir` (all for `""`).
fn below(entries: &[tree::Entry], dir: &str) -> Vec<tree::Entry> {
    entries
        .iter()
        .filter(|e| dir.is_empty() || e.path.strip_prefix(dir).is_some_and(|r| r.starts_with('/')))
        .cloned()
        .collect()
}

/// A local folder without anything inside `.git` (at any depth, any case),
/// walked once when made. A `limited` walk counts what [`path_safety::check`]
/// counts, the regular files that are not ignored, and stops at the first
/// one that takes the folder over a skill's limits, so a folder that grew far
/// past them is refused after about [`LIMITS`]`.files` entries.
struct LocalFolder {
    tree: WorkingTree,
    /// Sorted by path bytes. A stopped walk keeps the folders above every
    /// entry it holds.
    entries: Vec<tree::Entry>,
}

impl LocalFolder {
    fn new(dir: &Path, limited: bool) -> std::io::Result<LocalFolder> {
        let tree = WorkingTree::new(dir);
        let mut entries = Vec::new();
        let (mut files, mut bytes) = (0, 0);
        // Folders still to read, each already listed before its content.
        let mut folders = Vec::new();
        if tree.kind("")? == Some(Kind::Dir) {
            folders.push(String::new());
        }
        'walk: while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(tree.path(&folder))? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.eq_ignore_ascii_case(".git") {
                    continue;
                }
                let path = tree::join(&folder, &name);
                let kind = tree::kind_of(&std::fs::symlink_metadata(entry.path())?);
                entries.push(tree::Entry {
                    path: path.clone(),
                    kind,
                });
                match kind {
                    Kind::Dir => folders.push(path),
                    Kind::File { size, .. } if limited && !is_ignored(&path) => {
                        files += 1;
                        bytes += size;
                        if files > LIMITS.files
                            || size > LIMITS.file_bytes
                            || bytes > LIMITS.skill_bytes
                        {
                            break 'walk;
                        }
                    }
                    _ => {}
                }
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(LocalFolder { tree, entries })
    }
}

impl Tree for LocalFolder {
    fn kind(&self, path: &str) -> std::io::Result<Option<Kind>> {
        self.tree.kind(path)
    }

    fn walk(&self, dir: &str) -> std::io::Result<Vec<tree::Entry>> {
        Ok(below(&self.entries, dir))
    }

    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.tree.read(path)
    }
}

/// The treeish of folder `path` of `commit`. Refused (exit 1) when git
/// lists no folder at `path`, as when the upstream moved or deleted it; git
/// failing (a damaged cache) is an error with git's own message (exit 4).
fn folder(cache: &Cache, commit: &str, path: &str) -> Result<String, Failure> {
    let treeish = treeish(commit, path);
    // A `path` ending in `/` makes git list what is inside the folder, so
    // ask without it, and take only a record of the folder itself.
    let folder = path.trim_end_matches('/');
    if folder.is_empty() {
        return Ok(treeish);
    }
    let url = cache.url();
    let mut ls = cache.git();
    ls.args(["--literal-pathspecs", "ls-tree", "-z", commit, "--", folder]);
    let entry = git::run(ls, &format!("listing {path} at {commit} of {url}"))?;
    if entry.starts_with(b"040000 tree ") && entry.ends_with(format!("\t{folder}\0").as_bytes()) {
        return Ok(treeish);
    }
    Err(Failure::refused(format!(
        "{url} has no folder `{path}` at {commit}"
    )))
}

fn treeish(commit: &str, path: &str) -> String {
    if path.is_empty() {
        format!("{commit}^{{tree}}")
    } else {
        format!("{commit}:{path}")
    }
}

/// One record of `git ls-tree -l`.
struct Entry {
    record: String,
    mode: String,
    oid: String,
    /// `None` for trees and gitlinks (`-`).
    size: Option<u64>,
    path: String,
}

impl Entry {
    /// `<mode> SP <type> SP <oid> SP+ <size> TAB <path>`; the path is
    /// verbatim with `-z`. A path that is not UTF-8 is refused (exit 1).
    fn parse(record: &[u8], origin: &str) -> Result<Entry, Failure> {
        let unexpected = || {
            Failure::operational(format!(
                "unexpected `git ls-tree` output: {}",
                String::from_utf8_lossy(record)
            ))
        };
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .ok_or_else(unexpected)?;
        let head = std::str::from_utf8(&record[..tab]).map_err(|_| unexpected())?;
        let fields: Vec<&str> = head.split_whitespace().collect();
        let [mode, _kind, oid, size] = fields[..] else {
            return Err(unexpected());
        };
        let path = String::from_utf8(record[tab + 1..].to_vec()).map_err(|e| {
            Failure::refused(format!(
                "{origin} holds a path that is not valid UTF-8: {:?}",
                String::from_utf8_lossy(e.as_bytes())
            ))
        })?;
        Ok(Entry {
            record: String::from_utf8_lossy(record).into_owned(),
            mode: mode.to_owned(),
            oid: oid.to_owned(),
            size: size.parse().ok(),
            path,
        })
    }

    fn unexpected(&self) -> Failure {
        Failure::operational(format!("unexpected `git ls-tree` output: {}", self.record))
    }
}

/// The contents of `oids`, in order, through one `git cat-file --batch`.
pub(crate) fn cat_blobs<'a>(
    mut cmd: Command,
    oids: impl Iterator<Item = &'a str>,
) -> Result<Vec<Vec<u8>>, Failure> {
    let request: String = oids.map(|oid| format!("{oid}\n")).collect();
    let count = request.lines().count();
    let io = |e: std::io::Error| Failure::operational(format!("cannot read git objects: {e}"));
    cmd.args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| Failure::operational(format!("cannot run git (skiman requires git): {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let writer = std::thread::spawn(move || stdin.write_all(request.as_bytes()));
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut contents = Vec::with_capacity(count);
    for _ in 0..count {
        let mut header = String::new();
        stdout.read_line(&mut header).map_err(io)?;
        let size = header
            .trim_end()
            .split(' ')
            .nth(2)
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| {
                Failure::operational(format!("cannot read a git object: {}", header.trim_end()))
            })?;
        let mut content = vec![0; size + 1];
        stdout.read_exact(&mut content).map_err(io)?;
        content.pop();
        contents.push(content);
    }
    let _ = writer.join();
    let output = child.wait_with_output().map_err(io)?;
    if !output.status.success() {
        return Err(Failure::operational(format!(
            "reading git objects failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(contents)
}
