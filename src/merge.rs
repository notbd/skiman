//! The three-way merge of `update` (spec Merge semantics) in a temporary
//! bare git repository under `.skiman/tmp/`, run in the MERGE profile
//! (`git::merge_command`): no hooks, filters, external merge drivers, user
//! config or user attributes.
//!
//! Each side is committed from files held in memory (`git fast-import`, so
//! no clean filter, `.gitignore` or attribute applies), then
//! `git merge-tree --write-tree --merge-base` merges them with git's default
//! rename detection. Only objects are used: nothing is checked out.
//!
//! Conflicts are rebuilt from merge-tree's stage entries
//! (impl-notes git-fetch-and-merge.md 5.4): text conflicts through
//! `git merge-file -L` with the fixed labels, everything markers cannot
//! express as conflict records next to the file, or under
//! `.skiman-conflicts/` when the record's path is taken.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::failure::Failure;
use crate::git;
use crate::path_safety::Content;
use crate::rules::{CONFLICT_RECORD_SUFFIXES, CONFLICTS_DIR};
use crate::snapshot::{Snapshot, SnapshotFile, cat_blobs};

/// The labels of the two sides in conflict markers (spec Merge semantics).
const LOCAL_LABEL: &str = "local (skiman)";
const UPSTREAM_LABEL: &str = "upstream (skiman)";
/// git's `buffer_is_binary`: a NUL in the first 8000 bytes.
const BINARY_PROBE: usize = 8000;

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
    /// The merge has conflicts: the files of the result, with markers and
    /// conflict records written in.
    Conflicts {
        merged: Snapshot,
        conflicts: Vec<Conflict>,
    },
}

/// One conflict written into the merge result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The file the conflict is about, relative to the skill folder.
    pub path: String,
    pub kind: ConflictKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    /// Conflict markers in the file at `path`.
    Markers,
    /// Upstream's version is in the conflict record `record`; the local
    /// version (if any) stays at `path`.
    UpstreamVersion { record: String },
    /// Upstream deleted the file the local side changed; the empty
    /// `record` marks it.
    DeletedUpstream { record: String },
}

/// One file of the result before it is read: a blob of the merge
/// repository, or content built here.
#[derive(Debug, Clone)]
struct Item {
    executable: bool,
    content: Bytes,
}

#[derive(Debug, Clone)]
enum Bytes {
    Blob(String),
    Data(Vec<u8>),
}

/// `<mode> <oid>` of one side of a conflicted path.
#[derive(Debug, Clone)]
struct Side {
    executable: bool,
    oid: String,
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
        git::run_with_input(import, &stream, &what)?;
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
                let stages = records
                    .take_while(|r| !r.is_empty())
                    .map(parse_stage)
                    .collect::<Result<Vec<_>, _>>()?;
                self.write_conflicts(&tree, stages, ours, theirs)
            }
            _ => Err(Failure::operational(format!(
                "merging failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
        }
    }
}

impl MergeRepo {
    /// Rebuilds the result of a merge with conflicts from the result `tree`
    /// and the stage entries (impl-notes git-fetch-and-merge.md 5.4).
    fn write_conflicts(
        &self,
        tree: &str,
        stage_entries: Vec<(String, u8, Side)>,
        ours: &str,
        theirs: &str,
    ) -> Result<Outcome, Failure> {
        let result_entries = self.ls_tree(tree)?;
        let mut stages: BTreeMap<String, BTreeMap<u8, Side>> = BTreeMap::new();
        for (path, stage, side) in &stage_entries {
            // merge-ort moves the file side of a file/directory conflict to
            // `<path>~<label>` (label = the commit given, maybe `_<n>` added).
            let path = untilde(path, &[ours, theirs]);
            stages.entry(path).or_default().insert(*stage, side.clone());
        }
        let conflicted: BTreeSet<&str> = stage_entries.iter().map(|(p, ..)| p.as_str()).collect();
        let mut wanted: BTreeSet<String> =
            result_entries.iter().map(|(_, s)| s.oid.clone()).collect();
        wanted.extend(stage_entries.iter().map(|(_, _, s)| s.oid.clone()));
        let blobs: BTreeMap<String, Vec<u8>> = wanted
            .iter()
            .cloned()
            .zip(cat_blobs(self.git(), wanted.iter().map(String::as_str))?)
            .collect();
        let item = |side: &Side| Item {
            executable: side.executable,
            content: Bytes::Blob(side.oid.clone()),
        };

        let mut result: BTreeMap<String, Item> = result_entries
            .iter()
            .filter(|(path, _)| !conflicted.contains(path.as_str()))
            .map(|(path, side)| (path.clone(), item(side)))
            .collect();
        // (record path, the conflict's file, deletion marker?, content)
        let mut records: Vec<(String, String, bool, Item)> = Vec::new();
        let mut markers = Vec::new();
        let upstream_record = |path: &str| format!("{path}{}", CONFLICT_RECORD_SUFFIXES[0]);
        for (path, st) in &stages {
            match (st.get(&1), st.get(&2), st.get(&3)) {
                (base, Some(o), Some(t)) => {
                    let binary = [Some(o), Some(t), base]
                        .into_iter()
                        .flatten()
                        .any(|s| is_binary(&blobs[&s.oid]));
                    if binary {
                        result.insert(path.clone(), item(o));
                        records.push((upstream_record(path), path.clone(), false, item(t)));
                        continue;
                    }
                    let base_oid = match base {
                        Some(b) => b.oid.clone(),
                        None => self.empty_blob()?,
                    };
                    let (content, conflicts) = self.merge_file(&o.oid, &base_oid, &t.oid)?;
                    let executable = match base {
                        Some(b) if b.executable == o.executable => t.executable,
                        _ => o.executable,
                    };
                    result.insert(
                        path.clone(),
                        Item {
                            executable,
                            content: Bytes::Data(content),
                        },
                    );
                    if conflicts {
                        markers.push(path.clone());
                    }
                }
                (Some(_), Some(o), None) => {
                    result.insert(path.clone(), item(o));
                    records.push((
                        format!("{path}{}", CONFLICT_RECORD_SUFFIXES[1]),
                        path.clone(),
                        true,
                        Item {
                            executable: false,
                            content: Bytes::Data(Vec::new()),
                        },
                    ));
                }
                (_, None, Some(t)) => {
                    records.push((upstream_record(path), path.clone(), false, item(t)));
                }
                (None, Some(o), None) => {
                    result.insert(path.clone(), item(o));
                }
                (_, None, None) => {}
            }
        }
        // Upstream entries below a path where the local side keeps a file
        // (file/directory): upstream's versions become records.
        let below_a_file: Vec<String> = result
            .keys()
            .filter(|p| ancestors(p).any(|a| result.contains_key(a)))
            .cloned()
            .collect();
        for path in below_a_file {
            let item = result.remove(&path).expect("listed");
            records.push((upstream_record(&path), path, false, item));
        }

        let mut conflicts: Vec<Conflict> = markers
            .into_iter()
            .map(|path| Conflict {
                path,
                kind: ConflictKind::Markers,
            })
            .collect();
        records.sort_by(|a, b| a.0.cmp(&b.0));
        for (record, path, deleted, item) in records {
            let record = if is_blocked(&result, &record) {
                format!("{CONFLICTS_DIR}/{record}")
            } else {
                record
            };
            if is_blocked(&result, &record) {
                return Err(Failure::operational(format!(
                    "cannot write the conflict record {record} without replacing an existing entry"
                )));
            }
            result.insert(record.clone(), item);
            conflicts.push(Conflict {
                path,
                kind: if deleted {
                    ConflictKind::DeletedUpstream { record }
                } else {
                    ConflictKind::UpstreamVersion { record }
                },
            });
        }
        conflicts.sort_by(|a, b| a.path.cmp(&b.path));

        let files = result
            .into_iter()
            .map(|(path, item)| SnapshotFile {
                path,
                executable: item.executable,
                content: match item.content {
                    Bytes::Blob(oid) => blobs[&oid].clone(),
                    Bytes::Data(data) => data,
                },
            })
            .collect();
        let merged = Snapshot {
            files,
            submodules: Vec::new(),
            git_files: Vec::new(),
        };
        if conflicts.is_empty() {
            // Every stage entry merged (an exec-bit-only add/add keeps the local mode).
            return Ok(Outcome::Clean(merged));
        }
        Ok(Outcome::Conflicts { merged, conflicts })
    }

    /// The regular files of `treeish`: path, mode and blob id.
    fn ls_tree(&self, treeish: &str) -> Result<Vec<(String, Side)>, Failure> {
        let mut ls = self.git();
        ls.args(["ls-tree", "-r", "-z", "--full-tree", treeish]);
        let out = git::run(ls, "listing the merge result")?;
        out.split(|b| *b == 0)
            .filter(|r| !r.is_empty())
            .map(|record| {
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
                let head = String::from_utf8_lossy(&record[..tab]);
                let fields: Vec<&str> = head.split(' ').collect();
                let [mode, "blob", oid] = fields[..] else {
                    return Err(unexpected());
                };
                let executable = match mode {
                    "100644" => false,
                    "100755" => true,
                    _ => return Err(unexpected()),
                };
                Ok((
                    String::from_utf8_lossy(&record[tab + 1..]).into_owned(),
                    Side {
                        executable,
                        oid: oid.to_owned(),
                    },
                ))
            })
            .collect()
    }

    /// Writes the empty blob (the base of an add/add) and returns its id.
    fn empty_blob(&self) -> Result<String, Failure> {
        let mut hash = self.git();
        hash.args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::null());
        let out = git::run(hash, "writing an empty file to the merge repository")?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }

    /// Three-way merges one text file with the fixed labels; `true` when
    /// the result holds conflict markers.
    fn merge_file(&self, ours: &str, base: &str, theirs: &str) -> Result<(Vec<u8>, bool), Failure> {
        let mut cmd = self.git();
        cmd.args([
            "merge-file",
            "-p",
            "--object-id",
            "-L",
            LOCAL_LABEL,
            "-L",
            "base",
            "-L",
            UPSTREAM_LABEL,
            ours,
            base,
            theirs,
        ]);
        let output = cmd.output().map_err(|e| {
            Failure::operational(format!("cannot run git (skiman requires git): {e}"))
        })?;
        match output.status.code() {
            // The exit status is the number of conflicts (at most 127).
            Some(code @ 0..=127) => Ok((output.stdout, code > 0)),
            _ => Err(Failure::operational(format!(
                "merging a file failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
        }
    }
}

/// One `<mode> SP <oid> SP <stage> TAB <path>` record of `merge-tree -z`.
fn parse_stage(record: &[u8]) -> Result<(String, u8, Side), Failure> {
    let unexpected = || {
        Failure::operational(format!(
            "unexpected `git merge-tree` output: {}",
            String::from_utf8_lossy(record)
        ))
    };
    let tab = record
        .iter()
        .position(|b| *b == b'\t')
        .ok_or_else(unexpected)?;
    let head = String::from_utf8_lossy(&record[..tab]);
    let fields: Vec<&str> = head.split(' ').collect();
    let [mode, oid, stage] = fields[..] else {
        return Err(unexpected());
    };
    let executable = match mode {
        "100644" => false,
        "100755" => true,
        _ => return Err(unexpected()),
    };
    let stage = stage.parse().map_err(|_| unexpected())?;
    Ok((
        String::from_utf8_lossy(&record[tab + 1..]).into_owned(),
        stage,
        Side {
            executable,
            oid: oid.to_owned(),
        },
    ))
}

/// `path` without merge-ort's `~<label>` or `~<label>_<n>` suffix.
fn untilde(path: &str, labels: &[&str]) -> String {
    for label in labels {
        if let Some((head, tail)) = path.rsplit_once(&format!("~{label}")) {
            let suffix_ok = tail.is_empty()
                || tail
                    .strip_prefix('_')
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if suffix_ok {
                return head.to_owned();
            }
        }
    }
    path.to_owned()
}

fn is_binary(content: &[u8]) -> bool {
    content[..content.len().min(BINARY_PROBE)].contains(&0)
}

/// The folders above `path`, nearest last: `a/b/c` gives `a`, `a/b`.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(move |(i, _)| &path[..i])
}

/// `true` when writing a file at `path` would replace an entry of `files`:
/// the same path, a file where a folder above it must be, or a folder at it.
fn is_blocked(files: &BTreeMap<String, Item>, path: &str) -> bool {
    files.contains_key(path)
        || ancestors(path).any(|a| files.contains_key(a))
        || files
            .keys()
            .any(|p| p.strip_prefix(path).is_some_and(|r| r.starts_with('/')))
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
