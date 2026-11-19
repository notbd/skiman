//! Where write commands stage content before renaming it into place: one
//! folder per command in the canon, `.skiman/tmp/<ts>/` (spec Data
//! formats, Canon layout; clarifications 3, 4 and 6 on #1), and one per
//! replace for project mirrors, `.skiman/swap/<ts>/`, and in a project,
//! `<anchor>/.skiman-swap-<ts>/` (`docs/project-scope.md`, "Writing").
//!
//! Anything in those places that no running command staged is a **tmp
//! leftover** (distinct from "leftover records", exit 3). Opening a
//! [`Staging`] moves the tmp leftovers of its place to the command's backup,
//! then creates the command's own folder, which is removed when the
//! [`Staging`] is dropped, except for what a replace swapped out that is
//! still in `old/`.

use std::io;
use std::path::{Path, PathBuf};

use crate::backup::{self, Backups};
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::failure::Failure;
use crate::fsutil;
use crate::test_hook;

/// Prefix of a project's staging folder; discovery never enters one.
pub const PROJECT_PREFIX: &str = ".skiman-swap-";

/// Where [`Staging::swap`] sets aside, without a swapping rename, the entry
/// it replaces: `old/<rel>.skiman-prev`.
pub const PREV: &str = ".skiman-prev";

/// A command's own staging folder (disposable staging, except `old/`).
#[derive(Debug)]
pub struct Staging {
    dir: PathBuf,
    /// The folder every write through this staging stays in: the canon,
    /// or a project root.
    root: PathBuf,
}

/// How [`Staging::swap`] ended.
pub enum Swap {
    /// The new entry is in place, and the one it replaced at `old`;
    /// `flushed` failed when the folder it went into could not be flushed.
    Done {
        old: PathBuf,
        flushed: io::Result<()>,
    },
    /// The entry taken out was not the one expected: it is back. What came
    /// out of the target instead (maybe not the new one) stays at this path.
    Changed(PathBuf),
}

impl Staging {
    /// The command's staging in `.skiman/tmp/`, after its tmp leftovers went
    /// to `backups` ([`Staging::open`]), printing where; a leftover that
    /// cannot be moved stops the command with exit 4. Call while holding the
    /// repo lock, after every check that can refuse the command without
    /// writing (exit 1 or 2), and before its first write.
    pub fn begin(
        repo: &Canon,
        _lock: &CanonLock,
        backups: &mut Backups,
    ) -> Result<Staging, Failure> {
        let (tmp, saved) = (repo.tmp_dir(), Path::new("repo/.skiman/tmp"));
        // Moved by hand, tmp leftovers would be where no update run again
        // looks for what it swapped out: the fix is the backup's.
        let again =
            |why| Failure::operational(format!("{why}; fix that, then run the command again"));
        let say = &mut |line| out!("{line}");
        move_tmp_leftovers(backups, &tmp, "", repo.root(), saved, say).map_err(again)?;
        Staging::create(&tmp, "", repo.root())
    }

    /// The first time a command opens one in `parent`, moves the tmp
    /// leftovers there (with a `prefix`, its folders named `<prefix>…`;
    /// without, every entry) to `<backup>/<saved>/`, first finishing any
    /// move into a backup cut short inside one, and gives each line that
    /// says so to `say`. What they hold is never deleted, only empty
    /// folders; one that cannot be moved stays where it is, with those not
    /// reached yet, and a line says so, so it never stops a project item's
    /// replace. Then creates the staging folder ([`Staging::create`]).
    pub fn open(
        parent: &Path,
        prefix: &str,
        root: &Path,
        backups: &mut Backups,
        saved: &Path,
        mut say: impl FnMut(String),
    ) -> io::Result<Staging> {
        // Once a command: what its earlier replaces left there stays where they said.
        if backups.swept.insert(parent.to_owned())
            && let Err(why) = move_tmp_leftovers(backups, parent, prefix, root, saved, &mut say)
        {
            say(format!("{why}; what was not moved stays where it is"));
        }
        Staging::create(parent, prefix, root).map_err(|failure| io::Error::other(failure.message))
    }

    /// Creates `<parent>/<prefix><timestamp>/`, which, with a prefix, holds a
    /// `.gitignore` of `*` while it exists, so a project's git never picks it
    /// up. Refuses (exit 4) a `parent` reached through a link from `root`.
    fn create(parent: &Path, prefix: &str, root: &Path) -> Result<Staging, Failure> {
        let cannot = |why: String| {
            Failure::operational(format!(
                "cannot create a folder in {}: {why}",
                parent.display()
            ))
        };
        if let Some(why) = fsutil::broken_chain(root, &parent.join("new")) {
            return Err(cannot(why));
        }
        let dir =
            backup::fresh_named_dir(parent, prefix, 0o777).map_err(|e| cannot(e.to_string()))?;
        let staging = Staging {
            dir,
            root: root.to_owned(),
        };
        if !prefix.is_empty() {
            let ignore = staging.dir.join(".gitignore");
            std::fs::write(ignore, "*\n").map_err(|e| cannot(e.to_string()))?;
        }
        Ok(staging)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `old/`: where a replace stages the new entry, and swaps out the one
    /// it replaces until that is in the backup. Never disposable.
    pub fn old_dir(&self) -> PathBuf {
        self.dir.join("old")
    }

    /// Renames `from` (staged here, or an entry of the canon that `mv`
    /// renames) to `to`, where nothing may be: an entry that appeared there
    /// is never replaced ([`fsutil::place_new_or_rename`]). Every folder of
    /// `root` on the way to `to` must be a real folder. Fails with exit 4,
    /// never copying instead.
    pub fn place(&self, from: &Path, to: &Path) -> Result<(), Failure> {
        let fail = |why: String| {
            Failure::operational(format!(
                "cannot rename {} to {}: {why}",
                from.display(),
                to.display()
            ))
        };
        if let Some(why) = fsutil::broken_chain(&self.root, to) {
            return Err(fail(why));
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Failure::operational(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        fsutil::publish(from, to).map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => Failure::operational(format!(
                "{} appeared while skiman was writing; nothing was put in its place",
                to.display()
            )),
            io::ErrorKind::CrossesDevices => Failure::operational(fsutil::crossing(from, to)),
            _ => fail(e.to_string()),
        })
    }

    /// Replaces the entry at `target` with the new one staged at
    /// `old/<rel>`. The two swap places in one rename ([`fsutil::exchange`]),
    /// so `target` is never empty, and the replaced entry is at `old/<rel>`;
    /// where the filesystem has no such rename, `target` is first renamed to
    /// `old/<rel>.skiman-prev` within the filesystem, and the path is empty
    /// until the new one is renamed in. `expected` checks the entry taken
    /// out, before anything else is done with it; one it refuses goes back
    /// ([`Swap::Changed`]), and nothing is removed. A failure that leaves the two
    /// elsewhere says where each is. `scope` names the test hook between the
    /// two renames (`<scope>:set-aside:<target below root>`).
    pub fn swap(
        &self,
        rel: &str,
        target: &Path,
        scope: &str,
        mut expected: impl FnMut(&Path) -> bool,
    ) -> Result<Swap, Failure> {
        let (new, shown) = (self.old_dir().join(rel), target.display());
        let stuck = |e: io::Error, old: &Path, new: &Path| {
            Failure::operational(format!(
                "{shown} changed while skiman was writing and cannot be put back ({e}): it is at {}, and the new one at {}",
                old.display(),
                new.display()
            ))
        };
        match fsutil::exchange(&new, target) {
            Ok(()) if expected(&new) => {
                // Both folders the swap wrote: where the new one and the old one are.
                let flushed = fsutil::flush_path(target.parent().expect("named"))
                    .and_then(|()| fsutil::flush_path(new.parent().expect("named")));
                return Ok(Swap::Done { old: new, flushed });
            }
            // Swapped again: what came out is back.
            Ok(()) => fsutil::exchange(&new, target).map_err(|e| stuck(e, &new, target))?,
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                let prev = self.old_dir().join(format!("{rel}{PREV}"));
                self.place(target, &prev)
                    .inspect_err(|_| fsutil::remove_entry(&new))?;
                let at = target.strip_prefix(&self.root).unwrap_or(target).display();
                test_hook::at(scope, &format!("set-aside:{at}"));
                if expected(&prev) {
                    self.place(&new, target).map_err(|e| {
                        let (new, prev) = (new.display(), prev.display());
                        Failure::operational(format!(
                            "{}; the old {shown} is at {prev} and the new one at {new}",
                            e.message
                        ))
                    })?;
                    return Ok(Swap::Done {
                        old: prev,
                        flushed: Ok(()),
                    });
                }
                fsutil::publish(&prev, target).map_err(|e| stuck(e, &prev, &new))?;
            }
            Err(e) => {
                fsutil::remove_entry(&new);
                return Err(Failure::operational(match e.kind() {
                    io::ErrorKind::CrossesDevices => fsutil::crossing(target, &new),
                    _ => format!("cannot replace {shown}: {e}"),
                }));
            }
        }
        Ok(Swap::Changed(new))
    }
}

/// Removes the disposable staging, then every empty folder in it and the
/// folder itself once empty. What is still in `old/` stays (with the
/// folders above it, and the `.gitignore`) as a tmp leftover; only its
/// empty folders go.
impl Drop for Staging {
    fn drop(&mut self) {
        let (old, ignore) = (self.old_dir(), self.dir.join(".gitignore"));
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries
                .flatten()
                .filter(|e| e.path() != old && e.path() != ignore)
            {
                fsutil::remove_entry(&entry.path());
            }
        }
        fsutil::remove_empty_dirs(&old);
        if !old.exists() {
            let _ = std::fs::remove_file(ignore);
        }
        fsutil::remove_empty_dirs(&self.dir);
    }
}

fn move_tmp_leftovers(
    backups: &mut Backups,
    parent: &Path,
    prefix: &str,
    root: &Path,
    saved: &Path,
    say: &mut impl FnMut(String),
) -> Result<(), String> {
    // Not the place's if reached through a link; `create` reports that.
    if fsutil::broken_chain(root, &parent.join("new")).is_some() {
        return Ok(());
    }
    let leftovers = match list(parent) {
        Ok(leftovers) => leftovers,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot read {}: {e}", parent.display())),
    };
    let ours = |path: &&PathBuf| {
        let name = path.file_name().expect("entry has a name");
        prefix.is_empty()
            || (name.to_string_lossy().starts_with(prefix) && path.is_dir() && !path.is_symlink())
    };
    // Each line is said as it happens, and what moved is said even when a
    // later leftover stops the move.
    let mut moved = None;
    let result = leftovers.iter().filter(ours).try_for_each(|leftover| {
        let at = leftover.display();
        let failed = |e| format!("cannot move the tmp leftover {at} to a backup: {e}");
        // A move into a backup cut short inside it ends as it would have.
        let finished = |entry: &Path, copy: &Path| {
            say(format!(
                "deleted what was left of {}, cut short on its way to {}",
                entry.display(),
                copy.display()
            ));
        };
        backups
            .finish_cut_short(leftover, finished)
            .map_err(failed)?;
        // Empty folders and a `.gitignore` alone are no content.
        fsutil::remove_empty_dirs(leftover);
        if list(leftover).is_ok_and(|all| all == [leftover.join(".gitignore")]) {
            fsutil::remove_entry(leftover);
        }
        if std::fs::symlink_metadata(leftover).is_err() {
            return Ok(());
        }
        let name = leftover.file_name().expect("entry has a name");
        let dest = backups
            .move_in(leftover, &saved.join(name))
            .map_err(failed)?;
        moved = dest.parent().map(Path::to_owned);
        Ok(())
    });
    if let Some(moved) = moved {
        say(format!("moved tmp leftovers to {}", moved.display()));
    }
    result
}

/// Puts in place the new entry that a [`Staging::swap`] stopped between
/// its two renames left in the tmp leftover `leftover` for `rel`, the path
/// the caller is about to write: `old/<rel>`, with the old one at
/// `old/<rel>.skiman-prev` and nothing at `<root>/<rel>`. Renamed there
/// without replacing anything, and only into a folder that is there, so a
/// path skiman is not writing now, and what was removed around it, stay as
/// they are. The line that says so.
pub fn finish_swap(leftover: &Path, root: &Path, rel: &str) -> Option<String> {
    let (new, target) = (leftover.join("old").join(rel), root.join(rel));
    let prev = leftover.join("old").join(format!("{rel}{PREV}"));
    let stopped = prev.exists() && !target.exists();
    let clear = stopped && fsutil::broken_chain(root, &target).is_none();
    (clear && fsutil::publish(&new, &target).is_ok()).then(|| {
        let (target, new) = (target.display(), new.display());
        format!("put {target} in place from {new}, where a replace stopped part-way left it")
    })
}

/// The entries of `dir`, sorted by path.
pub fn list(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        out.push(entry?.path());
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A swap whose check fails while something else takes the target's
    /// place: swapped back, the original A is at the target and the entry
    /// that came out, B, stays in `old/` (never removed), a file or a folder.
    #[test]
    fn what_comes_out_of_a_swap_back_is_never_removed() {
        for b_is_a_folder in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let target = root.join("skills/pdf");
            std::fs::create_dir_all(&target).unwrap();
            std::fs::write(target.join("SKILL.md"), "A").unwrap();
            let mut backups = Backups::under(root.join("backups"));
            let parent = root.join(".skiman/tmp");
            let saved = Path::new("x");
            let staging = Staging::open(&parent, "", &root, &mut backups, saved, drop).unwrap();
            let staged = staging.old_dir().join("skills/pdf");
            std::fs::create_dir_all(&staged).unwrap();
            std::fs::write(staged.join("SKILL.md"), "new").unwrap();
            let swapped = staging.swap("skills/pdf", &target, "t", |_| {
                // B takes the target's place, where the new one went in.
                std::fs::remove_dir_all(&target).unwrap();
                match b_is_a_folder {
                    true => {
                        std::fs::create_dir(&target).unwrap();
                        std::fs::write(target.join("b.md"), "B").unwrap();
                    }
                    false => std::fs::write(&target, "B").unwrap(),
                }
                false
            });
            let Ok(Swap::Changed(at)) = swapped else {
                panic!("not swapped back");
            };
            assert_eq!(
                std::fs::read_to_string(target.join("SKILL.md")).unwrap(),
                "A"
            );
            let b = match b_is_a_folder {
                true => at.join("b.md"),
                false => at.clone(),
            };
            assert_eq!(std::fs::read_to_string(b).unwrap(), "B");
            drop(staging);
            assert!(at.exists(), "kept as a tmp leftover");
        }
    }

    /// The shape update calls it with: a skill folder. Only the path being
    /// written goes back, and only into its folder if that is still there:
    /// what was removed around it stays removed.
    #[test]
    fn finish_swap_puts_back_only_the_path_written() {
        let temp = tempfile::tempdir().unwrap();
        let temp = temp.path().canonicalize().unwrap();
        let (root, leftover) = (temp.join("root"), temp.join("tmp/1"));
        for (name, text) in [("pdf", "new"), (&format!("pdf{PREV}"), "old")] {
            let dir = leftover.join("old/skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), text).unwrap();
        }
        std::fs::create_dir(&root).unwrap();
        assert_eq!(finish_swap(&leftover, &root, "skills/other"), None);
        let gone = finish_swap(&leftover, &root, "skills/pdf");
        assert_eq!(gone, None, "skills/ is gone");
        std::fs::create_dir(root.join("skills")).unwrap();
        assert!(finish_swap(&leftover, &root, "skills/pdf").is_some());
        let put = std::fs::read_to_string(root.join("skills/pdf/SKILL.md")).unwrap();
        assert_eq!(put, "new");
    }

    /// A tmp leftover with `old/skills/pdf` but nothing at `pdf.skiman-prev`
    /// is no replace stopped between its two renames: after a swap,
    /// `old/skills/pdf` is the old skill. Nothing goes back.
    #[test]
    fn finish_swap_needs_the_old_one_set_aside() {
        let temp = tempfile::tempdir().unwrap();
        let temp = temp.path().canonicalize().unwrap();
        let (root, leftover) = (temp.join("root"), temp.join("tmp/1"));
        std::fs::create_dir_all(leftover.join("old/skills/pdf")).unwrap();
        std::fs::create_dir_all(root.join("skills")).unwrap();
        assert_eq!(finish_swap(&leftover, &root, "skills/pdf"), None);
        assert!(!root.join("skills/pdf").exists());
    }

    /// `skills/` turned into a link elsewhere: nothing goes back through it.
    #[test]
    fn finish_swap_never_puts_back_through_a_link() {
        let temp = tempfile::tempdir().unwrap();
        let temp = temp.path().canonicalize().unwrap();
        let (root, leftover) = (temp.join("root"), temp.join("tmp/1"));
        for name in ["pdf", &format!("pdf{PREV}")] {
            std::fs::create_dir_all(leftover.join("old/skills").join(name)).unwrap();
        }
        let elsewhere = temp.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("skills")).unwrap();
        assert_eq!(finish_swap(&leftover, &root, "skills/pdf"), None);
        assert!(!elsewhere.join("pdf").exists());
    }
}
