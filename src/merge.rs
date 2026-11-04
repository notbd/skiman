//! The three-way merge of `update` (spec Merge semantics) in a temporary
//! bare git repository under `.skiman/tmp/`, run in the MERGE profile
//! (`git::merge_command`): no hooks, filters, external merge drivers, user
//! config or user attributes.
//!
//! Each side is committed from files held in memory (`git fast-import`, so
//! no clean filter, `.gitignore` or attribute applies), then
//! `git merge-tree --write-tree --merge-base` merges them with git's default
//! rename detection. Only objects are used: nothing is checked out.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::failure::Failure;
use crate::git;
use crate::path_safety::Content;
use crate::snapshot::{Snapshot, SnapshotFile};

/// `$GIT_DIR/info/attributes` takes precedence over in-tree
/// `.gitattributes`; `!attr` resets each attribute that could change a merge.
const NEUTRAL_ATTRIBUTES: &str =
    "* !merge !conflict-marker-size !text !eol !crlf !diff !filter !ident !working-tree-encoding\n";

/// A temporary bare repository for merging one skill.
#[derive(Debug)]
pub struct MergeRepo {
    git_dir: PathBuf,
    home: PathBuf,
}

/// What `merge-tree` produced.
#[derive(Debug)]
pub enum Outcome {
    /// Every change merged: the files of the result.
    Clean(Snapshot),
    /// The merge has conflicts: the paths with stage entries (sorted,
    /// deduplicated). Writing them out is #15's.
    Conflicts(Vec<String>),
}

impl MergeRepo {
    /// Creates `<dir>/merge.git` (bare, no templates) and its neutral
    /// attributes; `dir` is the temporary `HOME` of every git call.
    pub fn create(dir: &Path) -> Result<MergeRepo, Failure> {
        let repo = MergeRepo {
            git_dir: dir.join("merge.git"),
            home: dir.to_path_buf(),
        };
        let mut init = repo.git();
        init.args(["init", "--quiet", "--bare", "--template=", "--"])
            .arg(&repo.git_dir);
        git::run(init, "creating the temporary merge repository")?;
        let attributes = repo.git_dir.join("info/attributes");
        std::fs::create_dir_all(attributes.parent().expect("has a parent"))
            .and_then(|()| std::fs::write(&attributes, NEUTRAL_ATTRIBUTES))
            .map_err(|e| {
                Failure::operational(format!("cannot write {}: {e}", attributes.display()))
            })?;
        Ok(repo)
    }

    fn git(&self) -> Command {
        git::merge_command(&self.git_dir, &self.home)
    }

    /// Commits `files` as a root commit on `refs/skiman/<side>` and returns its sha.
    pub fn commit(&self, side: &str, files: &[SnapshotFile]) -> Result<String, Failure> {
        let reference = format!("refs/skiman/{side}");
        let mut stream = Vec::new();
        stream.extend_from_slice(
            format!(
                "commit {reference}\ncommitter skiman <skiman@invalid> 0 +0000\ndata 0\ndeleteall\n"
            )
            .as_bytes(),
        );
        for file in files {
            let mode = if file.executable { "100755" } else { "100644" };
            stream.extend_from_slice(format!("M {mode} inline ").as_bytes());
            stream.extend_from_slice(&quote(&file.path));
            stream.extend_from_slice(format!("\ndata {}\n", file.content.len()).as_bytes());
            stream.extend_from_slice(&file.content);
            stream.push(b'\n');
        }
        let what = format!("committing the {side} side of the merge");
        let mut import = self.git();
        import.args(["fast-import", "--quiet"]);
        run_with_input(import, &stream, &what)?;
        let mut parse = self.git();
        parse.args(["rev-parse", "--verify", &reference]);
        let out = git::run(parse, &what)?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    /// Merges `ours` and `theirs` (commits) over `base`.
    pub fn merge(&self, base: &str, ours: &str, theirs: &str) -> Result<Outcome, Failure> {
        let mut cmd = self.git();
        cmd.args([
            "merge-tree",
            "--write-tree",
            "-z",
            &format!("--merge-base={base}"),
            ours,
            theirs,
        ]);
        let output = cmd.output().map_err(|e| {
            Failure::operational(format!("cannot run git (skiman requires git): {e}"))
        })?;
        let mut records = output.stdout.split(|b| *b == 0);
        let tree = String::from_utf8_lossy(records.next().unwrap_or_default()).into_owned();
        match output.status.code() {
            Some(0) => Ok(Outcome::Clean(Snapshot::read_tree(
                || self.git(),
                &tree,
                "the merge result",
                Content::Output,
            )?)),
            Some(1) => {
                let mut paths: Vec<String> = records
                    .take_while(|r| !r.is_empty())
                    .filter_map(|r| r.iter().position(|b| *b == b'\t').map(|t| &r[t + 1..]))
                    .map(|p| String::from_utf8_lossy(p).into_owned())
                    .collect();
                paths.dedup();
                Ok(Outcome::Conflicts(paths))
            }
            _ => Err(Failure::operational(format!(
                "merging failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
        }
    }
}

/// `path` as a C-style quoted string, the form `git fast-import` always accepts.
fn quote(path: &str) -> Vec<u8> {
    let mut out = vec![b'"'];
    for &b in path.as_bytes() {
        match b {
            b'"' | b'\\' => out.extend_from_slice(&[b'\\', b]),
            b'\n' => out.extend_from_slice(b"\\n"),
            0..0x20 | 0x7f => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            _ => out.push(b),
        }
    }
    out.push(b'"');
    out
}

fn run_with_input(mut cmd: Command, input: &[u8], what: &str) -> Result<(), Failure> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| Failure::operational(format!("cannot run git (skiman requires git): {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child
        .wait_with_output()
        .map_err(|e| Failure::operational(format!("{what} failed: {e}")))?;
    let _ = writer.join();
    if !output.status.success() {
        return Err(Failure::operational(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
