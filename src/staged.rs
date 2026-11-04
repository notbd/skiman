//! [`StagedTree`]: the staged snapshot (the git index) as a [`Tree`], so the
//! rules check exactly what a commit would record.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::tree::{Entry, Kind, Tree};

/// The index of the git repo holding `root`, limited to the entries below
/// `root`, with paths relative to it. Only stage-0 entries take part: an
/// unmerged path cannot be committed anyway.
pub struct StagedTree {
    root: std::path::PathBuf,
    /// Every entry, folders included, by path; the object id for files and symlinks.
    entries: BTreeMap<String, (Kind, Option<String>)>,
    cat_file: RefCell<Option<CatFile>>,
}

impl StagedTree {
    pub fn read(root: &Path) -> io::Result<StagedTree> {
        let output = git(root)
            .args([
                "ls-files",
                "-z",
                "--format=%(objectmode) %(objectname) %(objectsize) %(stage)\t%(path)",
            ])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "git ls-files failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let mut entries = BTreeMap::new();
        entries.insert(String::new(), (Kind::Dir, None));
        for record in output.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let record = String::from_utf8_lossy(record);
            let malformed = || io::Error::other(format!("unexpected git ls-files line: {record}"));
            let (fields, path) = record.split_once('\t').ok_or_else(malformed)?;
            let mut fields = fields.split(' ');
            let (Some(mode), Some(oid), Some(size), Some(stage)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(malformed());
            };
            if stage != "0" {
                continue;
            }
            let kind = match mode {
                "100644" | "100755" => Kind::File {
                    size: size.parse().map_err(|_| malformed())?,
                    executable: mode == "100755",
                },
                "120000" => Kind::Symlink,
                // A gitlink (submodule) is neither a file nor a folder of this repo.
                _ => Kind::Special,
            };
            let mut ancestor = path;
            while let Some((parent, _)) = ancestor.rsplit_once('/') {
                entries.insert(parent.to_owned(), (Kind::Dir, None));
                ancestor = parent;
            }
            entries.insert(path.to_owned(), (kind, Some(oid.to_owned())));
        }
        Ok(StagedTree {
            root: root.to_owned(),
            entries,
            cat_file: RefCell::new(None),
        })
    }
}

impl Tree for StagedTree {
    fn kind(&self, path: &str) -> io::Result<Option<Kind>> {
        Ok(self.entries.get(path).map(|(kind, _)| *kind))
    }

    fn walk(&self, dir: &str) -> io::Result<Vec<Entry>> {
        if self.entries.get(dir).map(|(kind, _)| *kind) != Some(Kind::Dir) {
            return Ok(Vec::new());
        }
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        Ok(self
            .entries
            .range(prefix.clone()..)
            .take_while(|(path, _)| path.starts_with(&prefix))
            .filter(|(path, _)| !path.is_empty())
            .map(|(path, (kind, _))| Entry {
                path: path.clone(),
                kind: *kind,
            })
            .collect())
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        let Some((Kind::File { .. }, Some(oid))) = self.entries.get(path) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} is not a staged file"),
            ));
        };
        let mut cat_file = self.cat_file.borrow_mut();
        if cat_file.is_none() {
            *cat_file = Some(CatFile::start(&self.root)?);
        }
        cat_file.as_mut().expect("started above").blob(oid)
    }
}

/// One `git cat-file --batch` process for every blob read.
struct CatFile {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl CatFile {
    fn start(root: &Path) -> io::Result<CatFile> {
        let mut child = git(root)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(CatFile {
            child,
            stdin,
            stdout,
        })
    }

    fn blob(&mut self, oid: &str) -> io::Result<Vec<u8>> {
        writeln!(self.stdin, "{oid}")?;
        self.stdin.flush()?;
        let mut header = String::new();
        self.stdout.read_line(&mut header)?;
        let size = match header.trim_end().split(' ').collect::<Vec<_>>()[..] {
            [_, "blob", size] => size.parse::<usize>().ok(),
            _ => None,
        }
        .ok_or_else(|| io::Error::other(format!("git cat-file: {}", header.trim_end())))?;
        let mut content = vec![0; size + 1];
        self.stdout.read_exact(&mut content)?;
        content.pop(); // the newline after the content
        Ok(content)
    }
}

impl Drop for CatFile {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// git in `root`. In a hook, git's own `GIT_DIR` and `GIT_INDEX_FILE` stay in
/// place: `git commit <paths>` checks a temporary index. Relative values are
/// made absolute, because they are relative to this process's directory, not `root`.
fn git(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(root);
    if let Ok(cwd) = std::env::current_dir() {
        for var in [
            "GIT_DIR",
            "GIT_INDEX_FILE",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
        ] {
            if let Some(value) = std::env::var_os(var).filter(|v| Path::new(v).is_relative()) {
                cmd.env(var, OsString::from(cwd.join(value)));
            }
        }
    }
    cmd
}
