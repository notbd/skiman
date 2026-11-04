//! Reading a folder of an upstream commit from the git cache by objects
//! (`ls-tree`, `cat-file`), never by checkout (spec Source fetching), so no
//! filter, hook or attribute of the upstream can run.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::Stdio;

use crate::failure::Failure;
use crate::fetch::Cache;
use crate::git;
use crate::tree::is_ignored;

/// One regular file of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFile {
    /// Relative to the snapshot folder, `/`-separated.
    pub path: String,
    pub executable: bool,
    pub content: Vec<u8>,
}

/// The files of one folder of an upstream commit, sorted by path bytes.
/// `.DS_Store` and `Thumbs.db` are left out (never copied into the skill repo).
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub files: Vec<SnapshotFile>,
    /// Submodules (gitlinks): not fetched, listed in the summary.
    pub submodules: Vec<String>,
}

impl Snapshot {
    pub fn file(&self, path: &str) -> Option<&SnapshotFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Reads folder `path` (`""` = the root) of `commit`, which must be in the cache.
    pub fn read(cache: &Cache, commit: &str, path: &str) -> Result<Snapshot, Failure> {
        let treeish = if path.is_empty() {
            format!("{commit}^{{tree}}")
        } else {
            format!("{commit}:{path}")
        };
        let mut ls = cache.git();
        ls.args(["ls-tree", "-r", "-t", "-z", "--full-tree", &treeish]);
        let listing = git::run(ls, &format!("listing {treeish} of {}", cache.url()))?;

        let mut snapshot = Snapshot::default();
        let mut blobs = Vec::new();
        for record in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let entry = Entry::parse(record, cache.url())?;
            check_path(&entry.path, cache.url())?;
            match entry.mode.as_str() {
                "040000" => {}
                "100644" | "100755" if is_ignored(&entry.path) => {}
                "100644" | "100755" => blobs.push(entry),
                "160000" => snapshot.submodules.push(entry.path),
                "120000" => {
                    return Err(Failure::refused(format!(
                        "{} holds a symlink at {}; symlinks are not allowed in skills",
                        cache.url(),
                        entry.path
                    )));
                }
                mode => {
                    return Err(Failure::refused(format!(
                        "{} holds {} with unsupported git mode {mode}",
                        cache.url(),
                        entry.path
                    )));
                }
            }
        }
        let contents = cat_blobs(cache, blobs.iter().map(|e| e.oid.as_str()))?;
        for (entry, content) in blobs.into_iter().zip(contents) {
            snapshot.files.push(SnapshotFile {
                executable: entry.mode == "100755",
                path: entry.path,
                content,
            });
        }
        snapshot.files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(snapshot)
    }
}

struct Entry {
    mode: String,
    oid: String,
    path: String,
}

impl Entry {
    /// `<mode> SP <type> SP <oid> TAB <path>`; the path is verbatim with `-z`.
    /// A path that is not UTF-8 is refused (exit 1).
    fn parse(record: &[u8], url: &str) -> Result<Entry, Failure> {
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
        let mut fields = head.split(' ');
        let (Some(mode), Some(_kind), Some(oid)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(unexpected());
        };
        let path = String::from_utf8(record[tab + 1..].to_vec()).map_err(|e| {
            Failure::refused(format!(
                "{url} holds a path that is not valid UTF-8: {:?}",
                String::from_utf8_lossy(e.as_bytes())
            ))
        })?;
        Ok(Entry {
            mode: mode.to_owned(),
            oid: oid.to_owned(),
            path,
        })
    }
}

/// Refuses a path that could leave the snapshot folder when written:
/// an empty component, `.`, `..` or `.git`. Full path safety (collisions,
/// LFS pointers, sizes) is applied on top of this by #13.
fn check_path(path: &str, url: &str) -> Result<(), Failure> {
    let unsafe_component = path
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == ".." || c.eq_ignore_ascii_case(".git"));
    if unsafe_component {
        return Err(Failure::refused(format!(
            "{url} holds an unsafe path: {path:?}"
        )));
    }
    Ok(())
}

/// The contents of `oids`, in order, through one `git cat-file --batch`.
fn cat_blobs<'a>(
    cache: &Cache,
    oids: impl Iterator<Item = &'a str>,
) -> Result<Vec<Vec<u8>>, Failure> {
    let request: String = oids.map(|oid| format!("{oid}\n")).collect();
    let count = request.lines().count();
    let io = |e: std::io::Error| {
        Failure::operational(format!("cannot read objects from the git cache: {e}"))
    };
    let mut cmd = cache.git();
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
                Failure::operational(format!(
                    "cannot read an object from the git cache: {}",
                    header.trim_end()
                ))
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
            "reading objects from the git cache failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(contents)
}
