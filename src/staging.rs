//! Where write commands stage content before renaming it into place: one
//! folder per command in the skill repo, `.skiman/tmp/<ts>/` (spec Data
//! formats, Skill repo layout; clarifications 3, 4 and 6 on #1), and one per
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
use crate::failure::Failure;
use crate::fsutil;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
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
    /// The folder every write through this staging stays in: the skill
    /// repo, or a project root.
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
        repo: &SkillRepo,
        _lock: &RepoLock,
        backups: &mut Backups,
    ) -> Result<Staging, Failure> {
        let (tmp, saved) = (repo.tmp_dir(), Path::new("repo/.skiman/tmp"));
        // Moved by hand, tmp leftovers would be where no update run again
        // looks for what it swapped out: the fix is the backup's.
        let again =
            |why| Failure::operational(format!("{why}; fix that, then run the command again"));
        for line in move_tmp_leftovers(backups, &tmp, "", repo.root(), saved).map_err(again)? {
            out!("{line}");
        }
        Staging::create(&tmp, "", repo.root())
    }

    /// The first time a command opens one in `parent`, moves the tmp
    /// leftovers there (with a `prefix`, its folders named `<prefix>…`;
    /// without, every entry) to `<backup>/<saved>/`, first finishing any
    /// move into a backup cut short inside one, and gives each line that
    /// says so to `say`. What they hold is never deleted, only empty
    /// folders; one that cannot be moved stays where it is, and a line says
    /// so, so it never stops a project item's replace. Then creates the
    /// staging folder ([`Staging::create`]).
    pub fn open(
        parent: &Path,
        prefix: &str,
        root: &Path,
        backups: &mut Backups,
        saved: &Path,
        say: impl FnMut(String),
    ) -> io::Result<Staging> {
        // Once a command: what its earlier replaces left there stays where they said.
        if backups.swept.insert(parent.to_owned()) {
            move_tmp_leftovers(backups, parent, prefix, root, saved)
                .unwrap_or_else(|why| vec![format!("{why}; the tmp leftovers stay where they are")])
                .into_iter()
                .for_each(say);
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

    /// Renames `from` (staged here, or an entry of the skill repo that `mv`
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
                            "{}; the old {shown} is at {prev} and the new one at {new}; rename the new one to {shown} before running the command again",
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
) -> Result<Vec<String>, String> {
    // Not the place's if reached through a link; `create` reports that.
    if fsutil::broken_chain(root, &parent.join("new")).is_some() {
        return Ok(Vec::new());
    }
    let leftovers = match list(parent) {
        Ok(leftovers) => leftovers,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", parent.display())),
    };
    let ours = |path: &&PathBuf| {
        let name = path.file_name().expect("entry has a name");
        prefix.is_empty()
            || (name.to_string_lossy().starts_with(prefix) && path.is_dir() && !path.is_symlink())
    };
    let leftovers: Vec<&PathBuf> = leftovers.iter().filter(ours).collect();
    if leftovers.is_empty() {
        return Ok(Vec::new());
    }
    let dir = backups.dir().map_err(|e| e.to_string())?;
    let (mut lines, mut moved) = (Vec::new(), false);
    for leftover in leftovers {
        let at = leftover.display();
        let failed = |e| format!("cannot move the tmp leftover {at} to a backup: {e}");
        // A move into a backup cut short inside it ends as it would have.
        for (entry, copy) in backups.finish_cut_short(leftover).map_err(failed)? {
            lines.push(format!(
                "deleted what was left of {}, cut short on its way to {}",
                entry.display(),
                copy.display()
            ));
        }
        // Empty folders and a `.gitignore` alone are no content.
        fsutil::remove_empty_dirs(leftover);
        if list(leftover).is_ok_and(|all| all == [leftover.join(".gitignore")]) {
            fsutil::remove_entry(leftover);
        }
        if std::fs::symlink_metadata(leftover).is_err() {
            continue;
        }
        let name = leftover.file_name().expect("entry has a name");
        backups
            .move_in(leftover, &saved.join(name))
            .map_err(failed)?;
        moved = true;
    }
    if moved {
        lines.push(format!(
            "moved tmp leftovers to {}",
            dir.join(saved).display()
        ));
    }
    Ok(lines)
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
}
