//! One write command's change to the canon: every piece as it should
//! be, in the order the command writes them (ADR 0005). A piece the repo
//! already holds as wanted is kept, so a command stopped part-way, killed
//! included, finishes when it is run again: nothing records how far it got,
//! every run works that out from what is on disk.
//!
//! Each piece goes in place in one step. A new folder or file is renamed in
//! without replacing anything; a folder that is there swaps places with its
//! replacement in one rename ([`fsutil::exchange`]), or, on a filesystem
//! without that rename, is renamed out of the way just before (the path is
//! empty only between the two renames); a file is replaced in one rename,
//! keeping its mode. What a piece replaces or removes goes to the backup.
//! Nothing is printed while writing: [`Change::write`] returns a line for
//! each step, and prints them itself only when the write stops.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use crate::backup::Backups;
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::failure::Failure;
use crate::fsutil;
use crate::snapshot::SnapshotFile;
use crate::staging::{Staging, Swap};
use crate::test_hook;
use crate::tree::{self, Overlay, WorkingTree};

/// What one piece of the canon should hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// A folder holding exactly these files.
    Folder(Vec<SnapshotFile>),
    /// A regular file with these bytes.
    File(Vec<u8>),
    /// Nothing: what is there goes to the backup.
    Absent,
    /// The entry at this repo path, renamed here (`mv`).
    From(String),
}

/// What the canon holds where a piece goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// Nothing there; for [`Want::From`], the entry is still at its old path.
    Nothing,
    /// Something not of the piece's kind; for [`Want::From`], entries at both paths.
    WrongType,
    /// The piece as wanted.
    Same,
    /// Something else: a file's bytes, a folder's tree hash.
    Other(Vec<u8>),
}

/// The pieces of one command's write, by path relative to the canon, in order.
pub struct Change {
    pub pieces: Vec<(String, Want)>,
    /// The command whose test hook names each write (`<command>:write:<path>`).
    pub command: &'static str,
    /// Saves a file's old bytes to the backup before replacing it (import).
    pub save_files: bool,
}

impl Change {
    /// The canon as it will be, for the rules: each folder piece holds
    /// exactly its files, each file piece its bytes, an absent one nothing.
    /// A renamed entry is not shown; `mv` checks its own.
    pub fn overlay<'a>(&self, tree: &'a WorkingTree) -> Overlay<'a> {
        let (mut files, mut executable, mut replaced) = (Vec::new(), BTreeSet::new(), Vec::new());
        for (rel, want) in &self.pieces {
            match want {
                Want::Folder(folder) => {
                    for file in folder {
                        let path = format!("{rel}/{}", file.path);
                        if file.executable {
                            executable.insert(path.clone());
                        }
                        files.push((path, file.content.clone()));
                    }
                    replaced.push(rel.clone());
                }
                Want::File(bytes) => files.push((rel.clone(), bytes.clone())),
                Want::Absent => replaced.push(rel.clone()),
                Want::From(_) => {}
            }
        }
        Overlay::new(tree, files)
            .with_executable(executable)
            .replacing(replaced)
    }

    /// Whether the canon at `root` holds every piece as wanted already.
    pub fn in_place(&self, root: &Path) -> Result<bool, Failure> {
        Ok(self.held(root)?.iter().all(|h| *h == Held::Same))
    }

    /// What the canon at `root` holds where each piece goes.
    pub fn held(&self, root: &Path) -> Result<Vec<Held>, Failure> {
        self.pieces
            .iter()
            .map(|(rel, want)| held(root, rel, want))
            .collect()
    }

    /// Whether writing over `held` puts anything in the backup: what a folder
    /// or absent piece replaces, and a file's old bytes with `save_files`.
    pub fn backs_up(&self, held: &[Held]) -> bool {
        self.pieces.iter().zip(held).any(|((_, want), seen)| {
            matches!(seen, Held::Other(_)) && (self.save_files || !matches!(want, Want::File(_)))
        })
    }

    /// [`Change::write`] over what the canon holds now (read under the
    /// canon lock), printing each line; whether anything went to the backup.
    pub fn apply(
        &self,
        repo: &Canon,
        lock: &CanonLock,
        backups: &mut Backups,
    ) -> Result<bool, Failure> {
        let held = self.held(repo.root())?;
        for line in self.write(&held, repo, lock, backups)? {
            out!("{line}");
        }
        Ok(self.backs_up(&held))
    }

    /// Writes, in order, each piece that `held` (read under the canon lock)
    /// does not show as wanted, staged in a folder of its own below
    /// `.skiman/tmp/` (tmp leftovers go to `backups` first); refuses, before
    /// anything is written, a piece that is [`Held::WrongType`]. A piece that
    /// changed since `held` saw it stops the write before it is touched.
    /// Returns a line for each step done. On an error the pieces written
    /// before it stay: their lines are printed, and its message says to run
    /// the same command again.
    pub fn write(
        &self,
        held: &[Held],
        repo: &Canon,
        lock: &CanonLock,
        backups: &mut Backups,
    ) -> Result<Vec<String>, Failure> {
        // A piece of the wrong kind stops the write before anything is written.
        let mut pieces = self.pieces.iter().zip(held);
        if let Some(((rel, _), _)) = pieces.find(|(_, seen)| **seen == Held::WrongType) {
            return Err(Failure::operational(format!(
                "{} is in the way: it is not of the kind skiman writes there; nothing was changed",
                repo.root().join(rel).display()
            )));
        }
        // The backup folder first, so one that cannot be made stops the write before it starts.
        if self.backs_up(held) {
            let nothing = |e| Failure::operational(format!("{e}; nothing was changed"));
            backups.dir().map_err(nothing)?;
        }
        let staging = Staging::begin(repo, lock, backups)?;
        let mut done = Vec::new();
        for ((rel, want), seen) in self.pieces.iter().zip(held) {
            if *seen == Held::Same {
                continue;
            }
            test_hook::at(self.command, &format!("write:{rel}"));
            // A piece that fails its checks is untouched; one that fails
            // while it is written may be part-way.
            let failed = match ready(repo.root(), rel, want, seen) {
                Err(failure) if done.is_empty() => Some(failure),
                Err(failure) => Some(stopped(failure)),
                Ok(()) => (self.put(repo.root(), rel, want, seen, &staging, backups, &mut done))
                    .err()
                    .map(stopped),
            };
            if let Some(failure) = failed {
                for line in &done {
                    out!("{line}");
                }
                return Err(failure);
            }
        }
        Ok(done)
    }

    #[allow(clippy::too_many_arguments)]
    fn put(
        &self,
        root: &Path,
        rel: &str,
        want: &Want,
        seen: &Held,
        staging: &Staging,
        backups: &mut Backups,
        done: &mut Vec<String>,
    ) -> Result<(), Failure> {
        let target = root.join(rel);
        let shown = target.display();
        let fail = |what: String| move |e: io::Error| Failure::operational(format!("{what}: {e}"));
        let saved = Path::new("repo").join(rel);
        // The line to print once `from` is in the backup.
        let move_in = |from: &Path, backups: &mut Backups| {
            let failed = match from == target {
                true => fail(format!("cannot move {shown} to the backup")),
                false => fail(format!(
                    "the new {shown} is in place, but the old one cannot be moved to the backup from {}, where it stays until the next write command moves it",
                    from.display()
                )),
            };
            let dest = backups.move_in(from, &saved).map_err(failed)?;
            Ok::<_, Failure>(format!("moved {shown} to {}", dest.display()))
        };
        match (want, seen) {
            (Want::Absent, _) => done.push(move_in(&target, backups)?),
            (Want::From(from), _) => {
                staging.place(&root.join(from), &target)?;
                done.push(format!("renamed {} to {shown}", root.join(from).display()));
            }
            (Want::File(bytes), Held::Nothing) => {
                let parent = target.parent().expect("a piece has a folder");
                std::fs::create_dir_all(parent)
                    .and_then(|()| fsutil::write_new(&target, bytes))
                    .map_err(fail(format!("cannot write {shown}")))?;
                done.push(format!("created {shown}"));
            }
            (Want::File(bytes), _) => {
                if let (true, Held::Other(old)) = (self.save_files, seen) {
                    // The backup is the only other copy once the file is replaced.
                    let dest = backups
                        .save(&saved, old)
                        .and_then(|dest| fsutil::barrier(&dest).map(|()| dest))
                        .map_err(fail(format!("cannot save the old {shown} to the backup")))?;
                    done.push(format!("saved the old {shown} as {}", dest.display()));
                }
                std::fs::metadata(&target)
                    .and_then(|meta| fsutil::rewrite_atomic(&target, bytes, meta.permissions()))
                    .map_err(fail(format!("cannot write {shown}")))?;
                done.push(format!("changed {shown}"));
            }
            (Want::Folder(files), Held::Nothing) => {
                let staged = staging.dir().join(rel);
                fsutil::write_files(&staged, files)?;
                staging.place(&staged, &target)?;
                done.push(format!("created {shown}"));
            }
            (Want::Folder(files), _) => {
                // Staged in `old/`, which a command never deletes: it holds
                // the new content until it is in place, then what was at
                // `target` until that is in the backup.
                let staged = staging.old_dir().join(rel);
                fsutil::write_files(&staged, files)
                    .inspect_err(|_| fsutil::remove_entry(&staged))?;
                let swapped = staging.swap(rel, &target, self.command, |old| {
                    let now = tree::tree_hash_of_dir(old);
                    matches!((seen, now), (Held::Other(was), Ok(now)) if was.as_slice() == now.as_bytes())
                })?;
                let (old, flushed) = match swapped {
                    Swap::Done { old, flushed } => (old, flushed),
                    Swap::Changed(new) => {
                        return Err(Failure::operational(format!(
                            "{shown} changed while skiman was writing; it is back where it was, and what came out is at {}",
                            new.display()
                        )));
                    }
                };
                done.push(format!("created {shown}"));
                done.push(move_in(&old, backups)?);
                flushed.map_err(fail(format!(
                    "the new {shown} is in place and the old one in the backup, but its folder cannot be flushed"
                )))?;
            }
        }
        Ok(())
    }
}

/// Refuses a piece whose path passes through a link, that is not of the
/// kind `want` is, or that changed since `seen` was read: a folder or file
/// by its content, an absent piece by whether it is there, a renamed one by
/// which of its two paths are there.
fn ready(root: &Path, rel: &str, want: &Want, seen: &Held) -> Result<(), Failure> {
    let target = root.join(rel);
    let shown = target.display();
    if let Some(why) = fsutil::broken_chain(root, &target) {
        return Err(Failure::operational(format!("cannot write {shown}: {why}")));
    }
    let now = held(root, rel, want)?;
    if now != *seen {
        let what = match now {
            Held::WrongType => "is in the way: it is not of the kind skiman writes there",
            _ => "changed while skiman was writing",
        };
        return Err(Failure::operational(format!(
            "{shown} {what}; nothing was put in its place"
        )));
    }
    Ok(())
}

/// What `root` holds at `rel` against `want`: a folder compares by tree
/// hash, a file by its bytes.
pub fn held(root: &Path, rel: &str, want: &Want) -> Result<Held, Failure> {
    let path = root.join(rel);
    let io = |e: io::Error| Failure::operational(format!("cannot read {}: {e}", path.display()));
    let meta = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(io(e)),
        Ok(meta) => Some(meta),
    };
    Ok(match (want, meta) {
        (Want::From(from), meta) => {
            let source = match std::fs::symlink_metadata(root.join(from)) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                there => there.map(|_| true).map_err(|e| {
                    let source = root.join(from);
                    Failure::operational(format!("cannot read {}: {e}", source.display()))
                })?,
            };
            match (source, meta) {
                (true, None) => Held::Nothing,
                (true, Some(_)) => Held::WrongType,
                (false, _) => Held::Same,
            }
        }
        (Want::Absent, None) => Held::Same,
        (Want::Absent, Some(_)) => Held::Other(Vec::new()),
        (_, None) => Held::Nothing,
        (Want::Folder(_), Some(meta)) if !meta.is_dir() => Held::WrongType,
        (Want::File(_), Some(meta)) if !meta.is_file() => Held::WrongType,
        (Want::Folder(files), Some(_)) => {
            let now = tree::tree_hash_of_dir(&path).map_err(io)?;
            let wanted = tree::files_hash(
                files
                    .iter()
                    .map(|f| (f.path.as_str(), f.executable, f.content.as_slice())),
            );
            match now == wanted {
                true => Held::Same,
                false => Held::Other(now.into_bytes()),
            }
        }
        (Want::File(bytes), Some(_)) => match std::fs::read(&path).map_err(io)? {
            now if now == *bytes => Held::Same,
            now => Held::Other(now),
        },
    })
}

/// `failure` of a command that stopped part-way, after its lines were
/// printed: running the same command again finishes it.
pub fn stopped(failure: Failure) -> Failure {
    Failure {
        message: format!(
            "{}\nthe command stopped part-way: the lines above list what was done; run the same command again to finish it",
            failure.message
        ),
        ..failure
    }
}
