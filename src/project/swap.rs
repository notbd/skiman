//! The replace of a project item side (`docs/project-scope.md`, "Writing"),
//! staged in a [`Staging`] on the destination's filesystem: `.skiman/swap/`
//! for the mirror, `<anchor>/.skiman-swap-<ts>/` for the project. The new
//! content and the replaced version swap places ([`Staging::swap`]), so a
//! set-aside copy keeps its inode and a late write through a handle opened
//! before the swap lands in it; only its move to a backup on another
//! filesystem copies it, verifies the copy and deletes it. Nothing replaced
//! is lost; an error never deletes the only copy.
//!
//! A file replaces an item that is there, so it is placed only where that
//! never replaces anything ([`fsutil::place_new`]): where it cannot be
//! (exFAT has neither a rename that never replaces nor hard links), a probe
//! in the staging folder fails first, and the destination is never touched.

use std::io;
use std::path::{Path, PathBuf};

use super::content::{self, Content};
use super::pattern::Kind;
use crate::backup::{self, Backups};
use crate::fsutil;
use crate::staging::{Staging, Swap};
use crate::test_hook;

/// What the plan saw at the destination of a replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// Nothing is there.
    Absent,
    /// An item side with this hash.
    Hash(String),
    /// Something that cannot be hashed as an item (a blocked project side):
    /// it is replaced as it is, and goes to the backup whole.
    Present,
}

impl Expect {
    pub fn from_hash(hash: Option<&str>) -> Expect {
        match hash {
            Some(hash) => Expect::Hash(hash.to_owned()),
            None => Expect::Absent,
        }
    }
}

/// Where the replaced version went, once the new content is in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Previous {
    /// Nothing was there.
    None,
    /// In the backup, at this path.
    At(PathBuf),
    /// It could not be moved to the backup (exit 4; never deleted): why,
    /// and where it is.
    Kept(String),
}

/// How a replace ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The new content is in place: record its hash. `late_write` is set
    /// when the replaced version changed after it was set aside (a write
    /// through a handle opened before the swap landed in it).
    Replaced {
        previous: Previous,
        late_write: bool,
    },
    /// The destination was not what the plan saw; nothing was replaced.
    Conflict(String),
    /// The new content is not in place, or its folder cannot be flushed;
    /// the message says where what was there is. Exit 4.
    Failed(String),
}

/// One replace of an item side.
pub struct Replace<'a> {
    pub dest: &'a Path,
    pub kind: Kind,
    pub expected: &'a Expect,
    pub new: &'a Content,
    /// Where the replaced version goes, relative to the backup folder.
    pub backup_rel: &'a Path,
}

impl Replace<'_> {
    /// Writes the new content at `dest`, below the root of `staging`; every
    /// folder on the way must be a real folder (no symlink), so a write
    /// never leaves the root or repo. The test hook `project:write:<path>`
    /// (relative to that root) runs first.
    pub fn run(&self, staging: &Staging, backups: &mut Backups) -> io::Result<Outcome> {
        let dest = self.dest;
        if let Some(why) = fsutil::broken_chain(staging.root(), dest) {
            return Ok(Outcome::Conflict(why));
        }
        let rel = dest.strip_prefix(staging.root()).expect("below the root");
        // One replace a staging, so the name alone tells the staged entry.
        let name = dest.file_name().expect("named").to_string_lossy();
        let rel = rel.to_string_lossy();
        test_hook::at("project", &format!("write:{rel}"));
        // Compared with the plan before anything is written; checked again
        // as it comes out of the swap.
        let now = content::hash_of(dest, self.kind).ok().flatten();
        let present = match std::fs::symlink_metadata(dest) {
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        let shown = dest.display();
        let conflict = match (present, self.expected) {
            (false, Expect::Absent) | (true, Expect::Present) => None,
            (false, _) => Some("disappeared"),
            (true, Expect::Absent) => Some("appeared"),
            (true, Expect::Hash(hash)) => (now.as_ref() != Some(hash)).then_some("changed"),
        };
        if let Some(how) = conflict {
            let why = format!("{shown} {how} since the plan was made");
            return Ok(Outcome::Conflict(why));
        }
        let staged = staging.old_dir().join(&*name);
        let discard = |outcome: Outcome| {
            fsutil::remove_entry(&staged);
            Ok(outcome)
        };
        if let Err(e) = write(&staged, self.new) {
            fsutil::remove_entry(&staged);
            return Err(e);
        }
        if self.kind == Kind::File {
            // Any error counts: exFAT answers ENOTSUP, which has no ErrorKind.
            let probe = staging.dir().join("probe");
            let probed = fsutil::place_new(&staged, &probe)
                .and_then(|()| fsutil::place_new(&probe, &staged));
            if let Err(e) = probed {
                return discard(Outcome::Failed(format!(
                    "cannot write {dest}: this volume cannot put a file in place without replacing what is there ({e}), so {dest} is unchanged",
                    dest = dest.display()
                )));
            }
        }
        if !present {
            return match staging.place(&staged, dest) {
                Ok(()) => Ok(Outcome::Replaced {
                    previous: Previous::None,
                    late_write: false,
                }),
                Err(failure) => discard(Outcome::Failed(failure.message)),
            };
        }
        // The baseline for the late-write check is taken as the replaced
        // version comes out, before anything else can happen to it; one that
        // cannot be read as an item counts as changed.
        let mut baseline = None;
        let swapped = staging.swap(&name, dest, "project", |old| {
            baseline = fingerprint(old).ok();
            let seen = content::hash_of(old, self.kind).ok().flatten();
            !matches!(self.expected, Expect::Hash(hash) if seen.as_ref() != Some(hash))
        });
        let (old, flushed) = match swapped {
            Ok(Swap::Done { old, flushed }) => (old, flushed),
            Ok(Swap::Changed(new)) => {
                let at = new.display();
                let why =
                    format!("{shown} changed since the plan was made (what came out is at {at})");
                return Ok(Outcome::Conflict(why));
            }
            Err(failure) => return Ok(Outcome::Failed(failure.message)),
        };
        let (previous, late_write, at) = match backups.move_in(&old, self.backup_rel) {
            // Only when both fingerprints are known and differ: an
            // unreadable set-aside copy (a blocked side) is no late write.
            Ok(to) => {
                let now = fingerprint(&to).ok();
                let late_write =
                    matches!((&baseline, now), (Some(before), Some(after)) if *before != after);
                let at = format!("the previous version is at {}", to.display());
                (Previous::At(to), late_write, at)
            }
            Err(e) => (Previous::Kept(kept(&e, &old)), false, kept(&e, &old)),
        };
        if let Err(e) = flushed {
            return Ok(Outcome::Failed(format!(
                "the new {shown} is in place, but its folder cannot be flushed ({e}); {at}"
            )));
        }
        Ok(Outcome::Replaced {
            previous,
            late_write,
        })
    }
}

/// Why the replaced version at `old` is not (all) in the backup (`e`), and
/// where it is.
fn kept(e: &io::Error, old: &Path) -> String {
    match (backup::is_kept(e), old.exists()) {
        // The backup holds all of it: what may be left is a part.
        (true, true) => format!(
            "moving its previous version to the backup did not finish ({e}); delete what is left of it at {}",
            old.display()
        ),
        (true, false) => format!("moving its previous version to the backup did not finish ({e})"),
        (false, _) => format!(
            "its previous version cannot be moved to the backup ({e}); it is kept at {}",
            old.display()
        ),
    }
}

/// A hash of whatever is at `path`, links not followed (the tree hash of a
/// folder, the content hash of a file): the late-write check compares it
/// before and after the move to the backup.
fn fingerprint(path: &Path) -> io::Result<String> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        crate::tree::tree_hash_of_dir(path)
    } else if meta.is_file() {
        Ok(Content::File(std::fs::read(path)?).hash())
    } else {
        Ok(String::new())
    }
}

fn write(path: &Path, content: &Content) -> io::Result<()> {
    let failure = |f: crate::failure::Failure| io::Error::other(f.message);
    match content {
        Content::File(bytes) => fsutil::write_file(path, false, bytes).map_err(failure),
        Content::Skill(files) => fsutil::write_files(path, files).map_err(failure),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use super::*;
    use crate::staging::PROJECT_PREFIX;

    struct World {
        _temp: tempfile::TempDir,
        root: PathBuf,
    }

    impl World {
        fn new() -> World {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            std::fs::create_dir_all(root.join("project")).unwrap();
            World { _temp: temp, root }
        }

        fn dest(&self) -> PathBuf {
            self.root.join("project/AGENTS.md")
        }

        fn backups(&self) -> Backups {
            Backups::under(self.root.join("backups"))
        }

        /// The staging of the `project` folder, as `project restore` opens it.
        fn staging(&self, backups: &mut Backups) -> Staging {
            let (anchor, saved) = (self.root.join("project"), Path::new("projects/w"));
            Staging::open(&anchor, PROJECT_PREFIX, &self.root, backups, saved, drop).unwrap()
        }

        fn replace(&self, expected: &Expect, backups: &mut Backups) -> Outcome {
            let staging = self.staging(backups);
            Replace {
                dest: &self.dest(),
                kind: Kind::File,
                expected,
                new: &Content::File(b"new\n".to_vec()),
                backup_rel: Path::new("projects/w/AGENTS.md"),
            }
            .run(&staging, backups)
            .unwrap()
        }

        fn swap_folders(&self) -> Vec<PathBuf> {
            let all = crate::staging::list(&self.root.join("project")).unwrap();
            let name = |p: &PathBuf| p.file_name().unwrap().to_string_lossy().into_owned();
            all.into_iter()
                .filter(|p| name(p).starts_with(PROJECT_PREFIX))
                .collect()
        }
    }

    fn hash(text: &str) -> Expect {
        Expect::Hash(Content::File(text.as_bytes().to_vec()).hash())
    }

    #[test]
    fn a_replace_puts_the_new_content_in_place_and_the_old_in_the_backup() {
        let world = World::new();
        std::fs::write(world.dest(), "old\n").unwrap();
        let mut backups = world.backups();
        let Outcome::Replaced {
            previous: Previous::At(at),
            late_write: false,
        } = world.replace(&hash("old\n"), &mut backups)
        else {
            panic!("not replaced");
        };
        assert_eq!(std::fs::read_to_string(world.dest()).unwrap(), "new\n");
        assert_eq!(std::fs::read_to_string(at).unwrap(), "old\n");
        assert!(world.swap_folders().is_empty());
    }

    #[test]
    fn a_changed_destination_is_a_conflict_and_stays_as_it_was() {
        let world = World::new();
        std::fs::write(world.dest(), "edited\n").unwrap();
        let inode = std::fs::metadata(world.dest()).unwrap().ino();
        let outcome = world.replace(&hash("old\n"), &mut world.backups());
        assert!(matches!(outcome, Outcome::Conflict(_)), "{outcome:?}");
        assert_eq!(std::fs::read_to_string(world.dest()).unwrap(), "edited\n");
        assert_eq!(std::fs::metadata(world.dest()).unwrap().ino(), inode);
        assert!(world.swap_folders().is_empty());
    }

    #[test]
    fn a_destination_that_appeared_is_a_conflict() {
        let world = World::new();
        std::fs::write(world.dest(), "someone's\n").unwrap();
        let outcome = world.replace(&Expect::Absent, &mut world.backups());
        assert!(matches!(outcome, Outcome::Conflict(_)), "{outcome:?}");
        assert_eq!(
            std::fs::read_to_string(world.dest()).unwrap(),
            "someone's\n"
        );
        assert!(world.swap_folders().is_empty());
    }

    #[test]
    fn a_backup_that_cannot_be_written_leaves_the_old_version_in_the_staging_folder() {
        let world = World::new();
        std::fs::write(world.dest(), "old\n").unwrap();
        let backups_dir = world.root.join("backups");
        std::fs::create_dir(&backups_dir).unwrap();
        std::fs::set_permissions(&backups_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let outcome = world.replace(&hash("old\n"), &mut world.backups());
        std::fs::set_permissions(&backups_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let Outcome::Replaced {
            previous: Previous::Kept(why),
            ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        // The new content is in place, so the caller records its hash.
        assert_eq!(std::fs::read_to_string(world.dest()).unwrap(), "new\n");
        let folders = world.swap_folders();
        let old = folders[0].join("old/AGENTS.md");
        assert!(
            why.ends_with(&format!("it is kept at {}", old.display())),
            "{why}"
        );
        assert_eq!(std::fs::read_to_string(old).unwrap(), "old\n");
        // The staging folder stays out of the project's git while it holds it.
        assert_eq!(
            std::fs::read_to_string(folders[0].join(".gitignore")).unwrap(),
            "*\n"
        );
    }

    #[test]
    fn a_new_entry_that_cannot_be_placed_fails_and_leaves_nothing_behind() {
        let world = World::new();
        let project = world.root.join("project");
        let mut backups = world.backups();
        let swap = world.root.join("swap");
        let staging =
            Staging::open(&swap, "", &world.root, &mut backups, Path::new("x"), drop).unwrap();
        std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o500)).unwrap();
        let dest = project.join("AGENTS.md");
        let outcome = Replace {
            dest: &dest,
            kind: Kind::File,
            expected: &Expect::Absent,
            new: &Content::File(b"x".to_vec()),
            backup_rel: Path::new("x"),
        }
        .run(&staging, &mut backups)
        .unwrap();
        drop(staging);
        std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
        assert!(!dest.exists());
        assert!(crate::staging::list(&swap).unwrap().is_empty());
    }

    #[test]
    fn a_link_on_the_way_refuses_the_write() {
        let world = World::new();
        let outside = world.root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, world.root.join("project/linked")).unwrap();
        let mut backups = world.backups();
        let staging = world.staging(&mut backups);
        let outcome = Replace {
            dest: &world.root.join("project/linked/AGENTS.md"),
            kind: Kind::File,
            expected: &Expect::Absent,
            new: &Content::File(b"x".to_vec()),
            backup_rel: Path::new("x"),
        }
        .run(&staging, &mut backups)
        .unwrap();
        assert!(matches!(outcome, Outcome::Conflict(_)), "{outcome:?}");
        assert!(!outside.join("AGENTS.md").exists());
    }

    #[test]
    fn leftovers_of_two_projects_never_share_a_backup_path() {
        let world = World::new();
        let mut backups = world.backups();
        for (project, text) in [("a", "only a\n"), ("b", "only b\n")] {
            let anchor = world.root.join(project);
            let old = anchor.join(".skiman-swap-20200101T000000Z/old");
            std::fs::create_dir_all(&old).unwrap();
            std::fs::write(old.join("AGENTS.md"), text).unwrap();
            let saved = Path::new("projects").join(project);
            let mut moved = Vec::new();
            let say = |l| moved.push(l);
            Staging::open(
                &anchor,
                PROJECT_PREFIX,
                &world.root,
                &mut backups,
                &saved,
                say,
            )
            .unwrap();
            assert!(!moved.is_empty());
        }
        let mut texts: Vec<String> = walk(backups.created().unwrap())
            .into_iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        texts.sort();
        assert_eq!(texts, ["only a\n", "only b\n"]);
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                out.extend(walk(&entry.path()));
            } else {
                out.push(entry.path());
            }
        }
        out
    }
}
