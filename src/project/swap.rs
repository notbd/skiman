//! The one protocol for every replace of a project item side (plan
//! "Writing"). Every temporary entry lives in a swap folder, one per
//! replaced item, on the destination's filesystem: `<repo>/.skiman/swap/<ts>/`
//! for the mirror, `<anchor>/.skiman-swap-<ts>/` for the project. Inside it,
//! `new/` holds the snapshot, `old/` the set-aside copy of what was there,
//! and `target` the destination path.
//!
//! A set-aside copy is only ever renamed, never copied and deleted: it keeps
//! its inode, so a late write through a handle opened before the swap lands
//! in it. Nothing replaced is deleted; an error never deletes the only copy.

use std::io;
use std::path::{Path, PathBuf};

use super::content::{self, Content};
use super::pattern::Kind;
use crate::backup::{fresh_dir, fresh_named_dir};
use crate::fsutil;

/// Prefix of a project-side swap folder; discovery never enters one.
pub const PROJECT_PREFIX: &str = ".skiman-swap-";

/// Which side a replace writes, and so where its swap folder lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place<'a> {
    /// In `swap_dir` (`<repo>/.skiman/swap`).
    Mirror { swap_dir: &'a Path },
    /// In the item's anchor, named `.skiman-swap-<ts>`, with a `.gitignore`
    /// holding `*` so a project repo never picks it up.
    Project { anchor: &'a Path },
}

impl Place<'_> {
    fn parent(&self) -> &Path {
        match self {
            Place::Mirror { swap_dir } => swap_dir,
            Place::Project { anchor } => anchor,
        }
    }
}

/// One command's backup folder (`<state dir>/backups/<ts>`), created when
/// first needed.
pub struct Backups {
    parent: PathBuf,
    dir: Option<PathBuf>,
}

impl Backups {
    /// Backups under `parent` (`<state dir>/backups`).
    pub fn under(parent: PathBuf) -> Backups {
        Backups { parent, dir: None }
    }

    fn dir(&mut self) -> io::Result<PathBuf> {
        if self.dir.is_none() {
            self.dir = Some(fresh_dir(&self.parent)?);
        }
        Ok(self.dir.clone().expect("just created"))
    }

    /// The backup folder, when this command created one.
    pub fn created(&self) -> Option<&Path> {
        self.dir.as_deref()
    }
}

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
    /// It could not be moved to the backup (another filesystem, an error)
    /// and stays at `at`, in its swap folder. Exit 4; never deleted.
    LeftInSwap { at: PathBuf, error: String },
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
    /// The new content is not in place; whatever was there is where it was,
    /// or the message says where it is kept. Exit 4.
    Failed(String),
}

/// One replace of an item side.
pub struct Replace<'a> {
    pub dest: &'a Path,
    pub kind: Kind,
    pub expected: &'a Expect,
    pub new: &'a Content,
    pub place: Place<'a>,
    /// Every folder from here down to `dest`'s parent must be a real folder
    /// (no symlink on the way), so a write never leaves the root or repo.
    pub inside: &'a Path,
    /// Where the replaced version goes, relative to the backup folder.
    pub backup_rel: &'a Path,
}

impl Replace<'_> {
    pub fn run(&self, backups: &mut Backups) -> io::Result<Outcome> {
        if let Some(why) = broken_chain(self.inside, self.dest) {
            return Ok(Outcome::Conflict(why));
        }
        let name = self
            .dest
            .file_name()
            .expect("item path has a name")
            .to_owned();
        let swap = create(self.place, self.dest)?;
        let staged = swap.join("new").join(&name);
        let old = swap.join("old").join(&name);
        if let Err(e) = write(&staged, self.new) {
            remove_swap(&swap);
            return Err(e);
        }

        let present = match std::fs::symlink_metadata(self.dest) {
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => {
                remove_swap(&swap);
                return Err(e);
            }
        };
        let mut set_aside = None;
        match (present, self.expected) {
            (false, Expect::Absent) => {}
            (false, _) => {
                remove_swap(&swap);
                return Ok(Outcome::Conflict(format!(
                    "{} disappeared since the plan was made",
                    self.dest.display()
                )));
            }
            (true, Expect::Absent) => {
                remove_swap(&swap);
                return Ok(Outcome::Conflict(format!(
                    "{} appeared since the plan was made",
                    self.dest.display()
                )));
            }
            (true, expected) => {
                if let Err(e) = std::fs::rename(self.dest, &old) {
                    remove_swap(&swap);
                    return Err(e);
                }
                // The baseline for the late-write check is taken right after
                // the set-aside, before anything else can happen to it.
                let baseline = fingerprint(&old).ok();
                // A set-aside copy that cannot be read counts as changed.
                let seen = content::hash_of(&old, self.kind).ok().flatten();
                let matches = match expected {
                    Expect::Hash(hash) => seen.as_deref() == Some(hash.as_str()),
                    _ => true,
                };
                if !matches {
                    return Ok(self.put_back(&old, &swap, |dest| {
                        Outcome::Conflict(format!("{dest} changed since the plan was made"))
                    }));
                }
                set_aside = Some(baseline);
            }
        }

        let placed = match self.dest.parent() {
            Some(dir) => std::fs::create_dir_all(dir),
            None => Ok(()),
        }
        .and_then(|()| place_without_replacing(&staged, self.dest, self.kind));
        if let Err(e) = placed {
            if set_aside.is_none() {
                remove_swap(&swap);
                return Ok(Outcome::Failed(format!(
                    "cannot put {} in place: {e}",
                    self.dest.display()
                )));
            }
            return Ok(self.put_back(&old, &swap, |dest| {
                Outcome::Failed(format!(
                    "cannot put the new {dest} in place ({e}); the previous one is back where it was"
                ))
            }));
        }

        let Some(baseline) = set_aside else {
            remove_swap(&swap);
            return Ok(Outcome::Replaced {
                previous: Previous::None,
                late_write: false,
            });
        };
        let moved = backups.dir().and_then(|dir| {
            let to = dir.join(self.backup_rel);
            std::fs::create_dir_all(to.parent().expect("backup path has a parent"))?;
            std::fs::rename(&old, &to)?;
            Ok(to)
        });
        let outcome = match moved {
            Ok(to) => {
                // Reported only when both fingerprints are known and differ:
                // an unreadable set-aside copy (a blocked side) is no late write.
                let late_write = matches!(
                    (&baseline, fingerprint(&to).ok()),
                    (Some(before), Some(after)) if *before != after
                );
                remove_swap(&swap);
                Outcome::Replaced {
                    previous: Previous::At(to),
                    late_write,
                }
            }
            Err(e) => Outcome::Replaced {
                previous: Previous::LeftInSwap {
                    at: old,
                    error: e.to_string(),
                },
                late_write: false,
            },
        };
        Ok(outcome)
    }

    /// Puts the set-aside copy back at `dest` without replacing anything,
    /// and answers with `outcome`; when the path was taken meanwhile, both
    /// versions stay and the answer says where.
    fn put_back(&self, old: &Path, swap: &Path, outcome: impl Fn(String) -> Outcome) -> Outcome {
        let dest = self.dest.display().to_string();
        match place_without_replacing(old, self.dest, self.kind) {
            Ok(()) => {
                remove_swap(swap);
                outcome(dest)
            }
            Err(e) => Outcome::Failed(format!(
                "{dest} could not be put back ({e}): something took its place; both are kept: {dest} and {}",
                old.display()
            )),
        }
    }
}

/// Why `dest` cannot be written: a component between `inside` and `dest`'s
/// parent that exists but is not a real folder. `None` when the way is clear
/// (components that do not exist yet are created later).
pub fn broken_chain(inside: &Path, dest: &Path) -> Option<String> {
    let rel = dest.parent()?.strip_prefix(inside).ok()?;
    let mut dir = inside.to_path_buf();
    for part in rel.components() {
        dir.push(part);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Some(format!(
                    "{} is not a real folder (a link or a file is in the way)",
                    dir.display()
                ));
            }
            Err(_) => return None,
        }
    }
    None
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

/// Puts `from` at `to` without replacing anything that is there: a hard
/// link for a file (fails if `to` exists), `rename` for a folder (fails over
/// a file or a non-empty folder).
fn place_without_replacing(from: &Path, to: &Path, kind: Kind) -> io::Result<()> {
    match kind {
        Kind::File => {
            std::fs::hard_link(from, to)?;
            std::fs::remove_file(from)
        }
        Kind::Skill => std::fs::rename(from, to),
    }
}

/// Creates a fresh swap folder for `dest`.
fn create(place: Place, dest: &Path) -> io::Result<PathBuf> {
    let prefix = match place {
        Place::Mirror { .. } => "",
        Place::Project { .. } => PROJECT_PREFIX,
    };
    let dir = fresh_named_dir(place.parent(), prefix)?;
    let filled = (|| {
        if let Place::Project { .. } = place {
            std::fs::write(dir.join(".gitignore"), "*\n")?;
        }
        std::fs::write(dir.join("target"), dest.display().to_string())?;
        std::fs::create_dir(dir.join("new"))?;
        std::fs::create_dir(dir.join("old"))
    })();
    if let Err(e) = filled {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(dir)
}

fn write(path: &Path, content: &Content) -> io::Result<()> {
    let failure = |f: crate::failure::Failure| io::Error::other(f.message);
    match content {
        Content::File(bytes) => fsutil::write_file(path, false, bytes).map_err(failure),
        Content::Skill(files) => fsutil::write_files(path, files).map_err(failure),
    }
}

/// Removes a swap folder whose `old/` is empty: `new/` is a copy of a
/// source that still exists, or a second link to an entry already in place.
fn remove_swap(swap: &Path) {
    if std::fs::read_dir(swap.join("old")).is_ok_and(|mut d| d.next().is_some()) {
        return;
    }
    let _ = std::fs::remove_dir_all(swap);
}

/// A swap folder an earlier command left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub dir: PathBuf,
    /// The destination it served, from its `target` file.
    pub target: Option<String>,
    /// Whether it still holds a set-aside copy (`old/` is not empty).
    pub holds_original: bool,
}

/// The swap folders of `place`: every folder in the mirror's swap folder,
/// the `.skiman-swap-*` folders of a project anchor.
pub fn leftovers(place: Place) -> Vec<Leftover> {
    let Ok(entries) = std::fs::read_dir(place.parent()) else {
        return Vec::new();
    };
    let mut out: Vec<Leftover> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| match place {
            Place::Mirror { .. } => true,
            Place::Project { .. } => e.file_name().to_string_lossy().starts_with(PROJECT_PREFIX),
        })
        .map(|e| {
            let dir = e.path();
            Leftover {
                target: std::fs::read_to_string(dir.join("target")).ok(),
                holds_original: std::fs::read_dir(dir.join("old"))
                    .is_ok_and(|mut d| d.next().is_some()),
                dir,
            }
        })
        .collect();
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// Moves what the swap folders of `place` hold in `old/` into the backup,
/// each into a fresh folder of its own under `swap-leftovers/` (never one
/// that exists), by rename only, then removes the folders. Returns one
/// message per folder; a copy that cannot be renamed (another filesystem)
/// stays, and its message says where.
pub fn recover(place: Place, backups: &mut Backups) -> Vec<String> {
    let mut messages = Vec::new();
    for leftover in leftovers(place) {
        let target = leftover
            .target
            .clone()
            .unwrap_or_else(|| "an item".to_owned());
        if !leftover.holds_original {
            remove_swap(&leftover.dir);
            continue;
        }
        let moved = (|| -> io::Result<PathBuf> {
            let to = fresh_dir(&backups.dir()?.join("swap-leftovers"))?;
            for entry in std::fs::read_dir(leftover.dir.join("old"))? {
                let entry = entry?;
                std::fs::rename(entry.path(), to.join(entry.file_name()))?;
            }
            Ok(to)
        })();
        match moved {
            Ok(to) => {
                remove_swap(&leftover.dir);
                messages.push(format!(
                    "moved the previous version of {target}, left by an interrupted write in {}, to {}",
                    leftover.dir.display(),
                    to.display()
                ));
            }
            Err(e) => messages.push(format!(
                "{} holds the previous version of {target} from an interrupted write and cannot be moved to the backup ({e}); move it by hand",
                leftover.dir.display()
            )),
        }
    }
    messages
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use super::*;

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

        fn replace(&self, expected: &Expect, backups: &mut Backups) -> Outcome {
            let dest = self.dest();
            let anchor = self.root.join("project");
            Replace {
                dest: &dest,
                kind: Kind::File,
                expected,
                new: &Content::File(b"new\n".to_vec()),
                place: Place::Project { anchor: &anchor },
                inside: &self.root,
                backup_rel: Path::new("projects/w/AGENTS.md"),
            }
            .run(backups)
            .unwrap()
        }

        fn swap_folders(&self) -> Vec<Leftover> {
            leftovers(Place::Project {
                anchor: &self.root.join("project"),
            })
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
    }

    #[test]
    fn a_backup_that_cannot_be_written_leaves_the_old_version_in_the_swap_folder() {
        let world = World::new();
        std::fs::write(world.dest(), "old\n").unwrap();
        let backups_dir = world.root.join("backups");
        std::fs::create_dir(&backups_dir).unwrap();
        std::fs::set_permissions(&backups_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let outcome = world.replace(&hash("old\n"), &mut world.backups());
        std::fs::set_permissions(&backups_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let Outcome::Replaced {
            previous: Previous::LeftInSwap { at, .. },
            ..
        } = outcome
        else {
            panic!("{outcome:?}");
        };
        // The new content is in place, so the caller records its hash.
        assert_eq!(std::fs::read_to_string(world.dest()).unwrap(), "new\n");
        assert_eq!(std::fs::read_to_string(at).unwrap(), "old\n");
        assert_eq!(world.swap_folders().len(), 1);
    }

    #[test]
    fn a_new_entry_that_cannot_be_placed_fails_and_leaves_nothing_behind() {
        let world = World::new();
        let project = world.root.join("project");
        std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o500)).unwrap();
        let dest = project.join("AGENTS.md");
        let outcome = Replace {
            dest: &dest,
            kind: Kind::File,
            expected: &Expect::Absent,
            new: &Content::File(b"x".to_vec()),
            place: Place::Mirror {
                swap_dir: &world.root.join("swap"),
            },
            inside: &world.root,
            backup_rel: Path::new("x"),
        }
        .run(&mut world.backups())
        .unwrap();
        std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
        assert!(!dest.exists());
        assert!(
            leftovers(Place::Mirror {
                swap_dir: &world.root.join("swap")
            })
            .is_empty()
        );
    }

    #[test]
    fn a_link_on_the_way_refuses_the_write() {
        let world = World::new();
        let outside = world.root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, world.root.join("project/linked")).unwrap();
        let dest = world.root.join("project/linked/AGENTS.md");
        let anchor = world.root.join("project");
        let outcome = Replace {
            dest: &dest,
            kind: Kind::File,
            expected: &Expect::Absent,
            new: &Content::File(b"x".to_vec()),
            place: Place::Project { anchor: &anchor },
            inside: &world.root,
            backup_rel: Path::new("x"),
        }
        .run(&mut world.backups())
        .unwrap();
        assert!(matches!(outcome, Outcome::Conflict(_)), "{outcome:?}");
        assert!(!outside.join("AGENTS.md").exists());
    }

    #[test]
    fn leftovers_of_two_projects_never_share_a_backup_path() {
        let world = World::new();
        let mut backups = world.backups();
        let mut moved = Vec::new();
        for (project, text) in [("a", "only a\n"), ("b", "only b\n")] {
            let anchor = world.root.join(project);
            let old = anchor.join(".skiman-swap-20200101T000000Z/old");
            std::fs::create_dir_all(&old).unwrap();
            std::fs::write(old.join("AGENTS.md"), text).unwrap();
            for message in recover(Place::Project { anchor: &anchor }, &mut backups) {
                moved.push(message);
            }
        }
        assert_eq!(moved.len(), 2);
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
