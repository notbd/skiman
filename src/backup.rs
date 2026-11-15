//! Backups under machine state (spec Data formats, Machine state):
//! `backups/<timestamp>/` with `repo/<path relative to the skill repo>` and
//! `<tool id>/<path relative to the tool directory>` inside, created when
//! first needed. A backup destination is always a new path. A move across
//! filesystems copies, compares the copy entry by entry, notes in
//! `<state dir>/moves/` where the copy is, then deletes the original
//! ([`Backups::move_in`]); the next move of what such a move left when it
//! was cut short deletes the rest and renames that copy to its own new path.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::fsutil::{self, entries};
use crate::paths::Paths;
use crate::test_hook;

/// UTC `YYYYMMDDTHHMMSSZ` of the current time.
pub fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Creates a new folder `<parent>/<timestamp>`, or `<timestamp>-2`, `-3`, ...
/// when that name exists. Never reuses an existing folder.
pub fn fresh_dir(parent: &Path) -> io::Result<PathBuf> {
    fresh_named_dir(parent, "", 0o777)
}

/// [`fresh_dir`] with `prefix` before the timestamp, created with `mode`
/// (the umask applies, as for `mkdir`).
pub fn fresh_named_dir(parent: &Path, prefix: &str, mode: u32) -> io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    let stamp = timestamp();
    for n in 1.. {
        let dir = if n == 1 {
            parent.join(format!("{prefix}{stamp}"))
        } else {
            parent.join(format!("{prefix}{stamp}-{n}"))
        };
        match std::fs::DirBuilder::new().mode(mode).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("the suffixes never run out")
}

/// One command's write context: its backup folder, `backups/<timestamp>/`,
/// created when the first thing goes in, so a command that backs nothing up
/// leaves no folder.
#[derive(Debug)]
pub struct Backups {
    parent: PathBuf,
    dir: Option<PathBuf>,
    /// Whether anything went in.
    used: bool,
    /// The places whose tmp leftovers this command moved (`Staging::open`).
    pub swept: BTreeSet<PathBuf>,
}

impl Backups {
    pub fn new(paths: &Paths) -> Backups {
        Backups::under(paths.state_dir().join("backups"))
    }

    /// Backups under `parent` instead of `<state dir>/backups`.
    pub fn under(parent: PathBuf) -> Backups {
        Backups {
            parent,
            dir: None,
            used: false,
            swept: BTreeSet::new(),
        }
    }

    /// The folder, created on first use.
    pub fn dir(&mut self) -> io::Result<PathBuf> {
        if self.dir.is_none() {
            let dir = fresh_dir(&self.parent).map_err(|e| {
                let parent = self.parent.display();
                io::Error::new(
                    e.kind(),
                    format!("cannot create a backup folder in {parent}: {e}"),
                )
            })?;
            self.dir = Some(dir);
        }
        Ok(self.dir.clone().expect("just created"))
    }

    /// The backup folder, once something went in.
    pub fn created(&self) -> Option<&Path> {
        self.dir.as_deref().filter(|_| self.used)
    }

    /// Writes `content`, the content of a file about to be replaced in one
    /// step, to `<backup>/<rel>`.
    pub fn save(&mut self, rel: &Path, content: &[u8]) -> io::Result<PathBuf> {
        let dest = self.fresh(rel)?;
        fsutil::write_new(&dest, content)?;
        self.used = true;
        Ok(dest)
    }

    /// `<backup>/<rel>` with its parent folders created; refuses a taken one.
    fn fresh(&mut self, rel: &Path) -> io::Result<PathBuf> {
        let dest = self.dir()?.join(rel);
        if std::fs::symlink_metadata(&dest).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", dest.display()),
            ));
        }
        std::fs::create_dir_all(dest.parent().expect("backup path has a parent"))?;
        Ok(dest)
    }

    /// Moves `original` to `<backup>/<rel>` (a symlink as a symlink) and
    /// returns where it is: one rename within a filesystem, else
    /// [`Backups::move_across`] (also for a folder from a lower overlayfs
    /// layer, which Linux cannot rename). What such a move left when it was
    /// cut short is finished: the rest deleted, its copy renamed here, the note last.
    pub fn move_in(&mut self, original: &Path, rel: &Path) -> io::Result<PathBuf> {
        let dest = self.fresh(rel)?;
        let folder = dest.parent().expect("a destination has a parent");
        if let Some(copy) = moved_copy(original, &self.parent) {
            delete(original).map_err(|e| kept(e, &copy))?;
            test_hook::at("move", "rest-deleted");
            fsutil::place_new_or_rename(&copy, &dest).map_err(|e| kept(e, &copy))?;
            self.used = true;
            fsutil::flush_path(folder).map_err(|e| kept(e, &dest))?;
            for empty in copy.ancestors().skip(1).take_while(|a| *a != self.parent) {
                let _ = std::fs::remove_dir(empty);
            }
            test_hook::at("move", "relocated");
            std::fs::remove_file(note_of(original, &self.parent)?).map_err(|e| kept(e, &dest))?;
            return Ok(dest);
        }
        match fsutil::place_new_or_rename(original, &dest) {
            Ok(()) => self.used = true,
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                return self.move_across(original, &dest).map(|()| dest);
            }
            Err(e) => return Err(e),
        }
        fsutil::flush_path(folder).map_err(|e| kept(e, &dest))?;
        Ok(dest)
    }

    /// Finishes each move to another filesystem that was cut short at or
    /// below `dir` (an entry with a [`moved_copy`]): deletes what is left of
    /// it, then its note. Its complete copy stays where that move made it.
    /// Returns each entry with its copy.
    pub fn finish_cut_short(&self, dir: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
        let mut finished = Vec::new();
        for (entry, _) in entries(dir)? {
            // Gone when it was in one finished before it.
            let Some(copy) = moved_copy(&entry, &self.parent) else {
                continue;
            };
            delete(&entry).map_err(|e| kept(e, &copy))?;
            std::fs::remove_file(note_of(&entry, &self.parent)?).map_err(|e| kept(e, &copy))?;
            finished.push((entry, copy));
        }
        Ok(finished)
    }

    /// Copies `from` next to `to` (`.<name>.skiman-partial`, which must not
    /// exist), flushes the copy, compares it with `from` entry by entry
    /// ([`inventory`]), renames it to `to` and drains it to the disk; then
    /// notes where the copy is ([`note_of`]) and deletes `from`, the folder
    /// after what it holds, and last the note. A write into `from` after the
    /// comparison is lost. An original holding a special file is refused
    /// and left in place.
    fn move_across(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        if let Some(special) = special_files(from)?.first() {
            return Err(io::Error::other(format!(
                "{} is a special file (FIFO, socket or device); move it away by hand",
                special.display()
            )));
        }
        let original = std::fs::symlink_metadata(from)?;
        let note = note_of(from, &self.parent)?;
        let mut partial = OsString::from(".");
        partial.push(to.file_name().expect("a destination has a name"));
        partial.push(".skiman-partial");
        let partial = to.with_file_name(partial);
        if std::fs::symlink_metadata(&partial).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} is in the way; nothing was moved", partial.display()),
            ));
        }
        let copied = copy_all(from, &partial)
            .and_then(|()| fsutil::flush_tree(&partial))
            .and_then(|()| same(&inventory(from)?, &inventory(&partial)?, from))
            .and_then(|()| fsutil::place_new_or_rename(&partial, to));
        if let Err(e) = copied {
            let _ = delete(&partial);
            return Err(e);
        }
        self.used = true;
        let mut text = format!("{} {}\n", original.dev(), original.ino()).into_bytes();
        text.extend_from_slice(to.as_os_str().as_bytes());
        let folder = to.parent().expect("a destination has a parent");
        fsutil::flush_path(folder)
            .and_then(|()| fsutil::barrier(folder))
            .and_then(|()| {
                test_hook::at("move", "copied");
                std::fs::create_dir_all(note.parent().expect("a note has a folder"))?;
                fsutil::write_atomic(&note, &text)
            })
            .and_then(|()| {
                test_hook::at("move", "noted");
                delete(from)
            })
            .and_then(|()| {
                test_hook::at("move", "deleted");
                std::fs::remove_file(&note)
            })
            .map_err(|e| kept(e, to))
    }
}

/// A backup folder nothing went in is removed when the command is done,
/// with the empty folders a failed move made in it; anything in it stays.
impl Drop for Backups {
    fn drop(&mut self) {
        if let (Some(dir), false) = (&self.dir, self.used) {
            fsutil::remove_empty_dirs(dir);
        }
    }
}

/// `e`, saying that the complete copy is at `copy`: the move made a
/// complete copy, and what is left of the original, if anything, is a part
/// ([`is_kept`] tells such an error apart).
fn kept(e: io::Error, copy: &Path) -> io::Error {
    let message = format!("{e}; the complete copy is at {}", copy.display());
    io::Error::new(e.kind(), Kept(message))
}

/// The message of a [`kept`] error.
#[derive(Debug)]
struct Kept(String);

impl std::fmt::Display for Kept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Kept {}

/// Whether `e`, from [`Backups::move_in`], says the backup holds a complete copy.
pub fn is_kept(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<Kept>())
}

/// Where a move across filesystems notes the complete copy it made of
/// `entry`, outside every folder the entry is in: `<state dir>/moves/`,
/// named by the `(dev, ino)` of the entry's folder and the entry's name in
/// hex, so every spelling of a path (through a link) finds it. It holds the
/// entry's own `(dev, ino)` on its first line and the copy's path after. It
/// describes this one entry; it is no record of a run (ADR 0005).
fn note_of(entry: &Path, backups: &Path) -> io::Result<PathBuf> {
    let folder = std::fs::metadata(entry.parent().expect("an entry has a folder"))?;
    let name = crate::tree::hex(entry.file_name().expect("an entry has a name").as_bytes());
    let key = format!("{}-{}-{name}", folder.dev(), folder.ino());
    Ok(backups.with_file_name("moves").join(key))
}

/// The copy the note for `entry` names, by the checks that read no content:
/// the note is a regular file whose first line is `entry`'s own `(dev, ino)`,
/// then an absolute path without `..`, below `backups`, there, and not
/// `entry` itself. Anything else is no note: `None`. Enough to tell what
/// `entry` is ([`crate::ownership`]); deleting it takes [`moved_copy`].
pub fn noted_copy(entry: &Path, backups: &Path) -> Option<PathBuf> {
    let note = note_of(entry, backups).ok()?;
    if !std::fs::symlink_metadata(&note).ok()?.is_file() {
        return None;
    }
    let text = std::fs::read(note).ok()?;
    let (identity, copy) = text.split_at(text.iter().position(|b| *b == b'\n')?);
    let copy = PathBuf::from(OsStr::from_bytes(&copy[1..]));
    let below = copy.is_absolute()
        && copy.starts_with(backups)
        && !copy.components().any(|c| c == Component::ParentDir);
    let held = std::fs::symlink_metadata(&copy).ok().filter(|_| below)?;
    let left = std::fs::symlink_metadata(entry).ok()?;
    let same_entry = format!("{} {}", left.dev(), left.ino()).as_bytes() == identity;
    (same_entry && (left.dev(), left.ino()) != (held.dev(), held.ino())).then_some(copy)
}

/// [`noted_copy`], when that copy also holds everything left of `entry` as
/// it is: what a move may delete.
pub fn moved_copy(entry: &Path, backups: &Path) -> Option<PathBuf> {
    noted_copy(entry, backups).filter(|copy| holds(copy, entry))
}

/// Whether `copy` holds everything at or below `entry` as it is; a special
/// file on either side is never read and fails it.
fn holds(copy: &Path, entry: &Path) -> bool {
    let (left, all) = (inventory(entry), inventory(copy));
    left.is_ok_and(|left| all.is_ok_and(|all| left.iter().all(|(p, i)| all.get(p) == Some(i))))
}

/// What doctor and adopt say of `entry`, left by a move cut short with its copy at `copy`.
pub fn cut_short(entry: &Path, copy: &Path) -> String {
    let then = match holds(copy, entry) {
        true => "delete it",
        false => "it changed since, so compare it with the copy before you delete it",
    };
    let (entry, copy) = (entry.display(), copy.display());
    format!(
        "{entry} is what a move to a backup left when it was cut short; its complete copy is at {copy}; {then}"
    )
}

/// What a move across filesystems compares, per entry at or below a root,
/// by its path relative to the root (bytes, so a name that is not UTF-8
/// compares as it is): its kind, a link's target, a file's content hash and
/// execute bits.
#[derive(Debug, PartialEq, Eq)]
enum Item {
    Folder,
    Link(Vec<u8>),
    File { executable: u32, sha256: Vec<u8> },
}

/// Only regular files are read: a special file (a FIFO would block the
/// read) fails the inventory.
fn inventory(root: &Path) -> io::Result<BTreeMap<Vec<u8>, Item>> {
    let mut out = BTreeMap::new();
    for (path, meta) in entries(root)? {
        let rel = path.strip_prefix(root).expect("below the root");
        let item = if meta.is_symlink() {
            Item::Link(std::fs::read_link(&path)?.into_os_string().into_vec())
        } else if meta.is_dir() {
            Item::Folder
        } else if meta.is_file() {
            let sha256 = Sha256::digest(std::fs::read(&path)?).to_vec();
            let executable = meta.mode() & 0o111;
            Item::File { executable, sha256 }
        } else {
            return Err(io::Error::other(format!(
                "{} is a special file",
                path.display()
            )));
        };
        out.insert(rel.as_os_str().as_bytes().to_vec(), item);
    }
    Ok(out)
}

/// Fails, naming the first path that differs, unless the copy holds
/// exactly what `original` holds.
fn same(
    original: &BTreeMap<Vec<u8>, Item>,
    copy: &BTreeMap<Vec<u8>, Item>,
    from: &Path,
) -> io::Result<()> {
    let differing = original
        .keys()
        .chain(copy.keys())
        .find(|k| original.get(*k) != copy.get(*k));
    match differing {
        None => Ok(()),
        Some(path) => Err(io::Error::other(format!(
            "the copy of {} differs from it (`{}`); nothing was deleted",
            from.display(),
            String::from_utf8_lossy(path)
        ))),
    }
}

/// Deletes `path`; a folder deepest entries first and itself last.
fn delete(path: &Path) -> io::Result<()> {
    if !std::fs::symlink_metadata(path)?.is_dir() {
        return std::fs::remove_file(path);
    }
    for (entry, meta) in entries(path)?.into_iter().rev() {
        match meta.is_dir() {
            true => std::fs::remove_dir(&entry)?,
            false => std::fs::remove_file(&entry)?,
        }
        test_hook::at("move", "deleting");
    }
    Ok(())
}

/// Every special file (FIFO, socket, device) at or below `path`, never
/// following symlinks: what a move across filesystems refuses.
pub fn special_files(path: &Path) -> io::Result<Vec<PathBuf>> {
    Ok(entries(path)?
        .into_iter()
        .filter(|(_, meta)| {
            let kind = meta.file_type();
            !(kind.is_file() || kind.is_dir() || kind.is_symlink())
        })
        .map(|(entry, _)| entry)
        .collect())
}

/// Where `entry` (at or below `from`) goes below `to`; `from` itself goes to
/// `to` (`to.join("")` would add a trailing slash).
fn copy_path(from: &Path, to: &Path, entry: &Path) -> PathBuf {
    let rel = entry.strip_prefix(from).expect("entry is below from");
    if rel.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rel)
    }
}

fn copy_all(from: &Path, to: &Path) -> io::Result<()> {
    for (entry, meta) in entries(from)? {
        let target = copy_path(from, to, &entry);
        if meta.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&entry)?, &target)?;
        } else if meta.is_dir() {
            std::fs::create_dir(&target)?;
        } else {
            std::fs::copy(&entry, &target)?;
            std::fs::set_permissions(
                &target,
                std::fs::Permissions::from_mode(meta.permissions().mode()),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder with every kind of entry a copy has to keep, in
    /// `<dir>/tool/skill`: a `SKILL.md`, an executable in a subfolder, an
    /// empty folder and a link.
    fn original(dir: &Path) -> PathBuf {
        let root = dir.join("tool/skill");
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir(root.join("empty")).unwrap();
        std::fs::write(root.join("SKILL.md"), "---\nname: skill\n---\n").unwrap();
        std::fs::write(root.join("scripts/run.sh"), "#!/bin/sh\n").unwrap();
        let run = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(root.join("scripts/run.sh"), run).unwrap();
        std::os::unix::fs::symlink("SKILL.md", root.join("link")).unwrap();
        root
    }

    /// `move_across` as a backup on another filesystem runs it.
    fn across(dir: &Path, from: &Path) -> (Backups, io::Result<()>, PathBuf) {
        let mut backups = Backups::under(dir.join("backups"));
        let to = backups.fresh(Path::new("tool/skill")).unwrap();
        let moved = backups.move_across(from, &to);
        (backups, moved, to)
    }

    fn fifo(path: &Path) {
        let made = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .unwrap();
        assert!(made.success());
    }

    /// Writes the note a move of `entry` to `copy` leaves, with `identity`
    /// for its first line (`None`: the entry's own `(dev, ino)`).
    fn note(entry: &Path, copy: &Path, backups: &Path, identity: Option<&str>) -> PathBuf {
        let path = note_of(entry, backups).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let meta = std::fs::symlink_metadata(entry).unwrap();
        let own = format!("{} {}", meta.dev(), meta.ino());
        let text = format!("{}\n{}", identity.unwrap_or(&own), copy.display());
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_move_across_filesystems_copies_every_entry_then_deletes_the_original() {
        let temp = tempfile::tempdir().unwrap();
        let from = original(temp.path());
        let before = inventory(&from).unwrap();
        let note = note_of(&from, &temp.path().join("backups")).unwrap();

        let (backups, moved, to) = across(temp.path(), &from);

        moved.unwrap();
        assert_eq!(inventory(&to).unwrap(), before);
        assert!(std::fs::symlink_metadata(&from).is_err());
        assert!(std::fs::symlink_metadata(note).is_err());
        let names: Vec<_> = std::fs::read_dir(to.parent().unwrap()).unwrap().collect();
        assert_eq!(names.len(), 1, "no partial copy is left");
        assert!(backups.created().is_some());
    }

    #[test]
    fn a_copy_differs_from_its_original_in_any_entry_kind_or_execute_bit() {
        let temp = tempfile::tempdir().unwrap();
        let from = original(temp.path());
        let copy = temp.path().join("copy");
        copy_all(&from, &copy).unwrap();
        let check = |copy: &Path| same(&inventory(&from)?, &inventory(copy)?, &from);
        check(&copy).unwrap();
        type Edit = fn(&Path);
        let edits: [(&str, Edit); 4] = [
            ("empty", |c| std::fs::remove_dir(c.join("empty")).unwrap()),
            ("scripts/run.sh", |c| {
                let plain = std::fs::Permissions::from_mode(0o644);
                std::fs::set_permissions(c.join("scripts/run.sh"), plain).unwrap();
            }),
            ("link", |c| {
                std::fs::remove_file(c.join("link")).unwrap();
                std::os::unix::fs::symlink("other", c.join("link")).unwrap();
            }),
            ("extra", |c| std::fs::write(c.join("extra"), "").unwrap()),
        ];
        for (path, edit) in edits {
            let changed = temp
                .path()
                .join(format!("changed-{}", path.replace('/', "-")));
            copy_all(&copy, &changed).unwrap();
            edit(&changed);
            let e = check(&changed).unwrap_err().to_string();
            assert!(e.contains(&format!("(`{path}`)")), "{path}: {e}");
        }
    }

    /// The step matrix: an original killed after each step of a move across
    /// filesystems, then moved again by the next command. Before the note
    /// is in place it is copied again (a second complete copy at worst);
    /// from then on (note written, each entry deleted) the rest is deleted
    /// and the copy renamed to this move's path, its emptied folders gone.
    /// With the original gone there is nothing to move: nothing changes.
    #[test]
    fn a_move_cut_short_at_any_step_is_finished_by_the_next_move() {
        let temp = tempfile::tempdir().unwrap();
        let steps = entries(&original(&temp.path().join("count")))
            .unwrap()
            .len();
        let mut states = vec!["partial", "copied", "note being written"];
        states.extend(std::iter::repeat_n("noted", steps + 1));
        for (step, state) in states.into_iter().enumerate() {
            let dir = temp.path().join(step.to_string());
            let backups_dir = dir.join("backups");
            let from = original(&dir);
            let before = inventory(&from).unwrap();
            let first = backups_dir.join("20260101T000000Z/tool/skill");
            std::fs::create_dir_all(first.parent().unwrap()).unwrap();
            match state {
                "partial" => {
                    let partial = first.with_file_name(".skill.skiman-partial");
                    std::fs::create_dir(&partial).unwrap();
                    std::fs::write(partial.join("SKILL.md"), "half").unwrap();
                }
                _ => copy_all(&from, &first).unwrap(),
            }
            let note_path = match state {
                "noted" => note(&from, &first, &backups_dir, None),
                "note being written" => {
                    let path = note_of(&from, &backups_dir).unwrap();
                    let tmp = path.with_file_name(".x.skiman-tmp-1");
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(tmp, "1 2\n/ha").unwrap();
                    path
                }
                _ => note_of(&from, &backups_dir).unwrap(),
            };
            if state == "noted" {
                let order = entries(&from).unwrap().into_iter().rev();
                for (path, meta) in order.take(step - 3) {
                    match meta.is_dir() {
                        true => std::fs::remove_dir(path).unwrap(),
                        false => std::fs::remove_file(path).unwrap(),
                    }
                }
            }
            let gone = std::fs::symlink_metadata(&from).is_err();
            let mut backups = Backups::under(backups_dir.clone());

            let moved = backups.move_in(&from, Path::new("tool/skill"));

            if gone {
                assert!(moved.is_err() && note_path.is_file(), "step {step}");
                assert_eq!(inventory(&first).unwrap(), before, "step {step}");
                continue;
            }
            let at = moved.unwrap();
            let finishes = state == "noted";
            assert!(std::fs::symlink_metadata(&from).is_err(), "step {step}");
            assert!(
                std::fs::symlink_metadata(&note_path).is_err(),
                "step {step}"
            );
            assert!(at.starts_with(backups.created().unwrap()), "step {step}");
            let old = backups_dir.join("20260101T000000Z");
            assert_eq!(old.exists(), !finishes, "step {step} ({state})");
            assert_eq!(inventory(&at).unwrap(), before, "step {step}");
        }
    }

    /// A note that does not check out is no note at all: the entry moves
    /// whole (here within one filesystem) and nothing is deleted.
    #[test]
    fn a_note_that_does_not_check_out_is_ignored_and_deletes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let backups_dir = temp.path().join("backups");
        let gone = backups_dir.join("20260101T000000Z/tool/skill");
        std::fs::create_dir_all(gone.parent().unwrap()).unwrap();
        let elsewhere = temp.path().join("elsewhere");
        copy_all(&original(&temp.path().join("model")), &elsewhere).unwrap();
        let cases = [
            "empty",
            "itself",
            "relative",
            "outside the backups",
            "a copy that is gone",
            "another entry of that name",
            "a FIFO",
        ];
        for (i, case) in cases.into_iter().enumerate() {
            let from = original(&temp.path().join(i.to_string()));
            let before = inventory(&from).unwrap();
            let named = match case {
                "itself" => from.clone(),
                "relative" => PathBuf::from("."),
                "outside the backups" => elsewhere.clone(),
                _ => gone.clone(),
            };
            let identity = (case == "another entry of that name").then_some("1 2");
            let path = note(&from, &named, &backups_dir, identity);
            match case {
                "empty" => std::fs::write(&path, "").unwrap(),
                "a FIFO" => {
                    std::fs::remove_file(&path).unwrap();
                    fifo(&path);
                }
                _ => {}
            }
            if case == "another entry of that name" {
                copy_all(&from, &gone).unwrap();
            }
            assert_eq!(moved_copy(&from, &backups_dir), None, "{case}");

            let mut backups = Backups::under(backups_dir.clone());
            let rel = Path::new(&i.to_string()).join("skill");
            let at = backups.move_in(&from, &rel).unwrap();

            assert_eq!(inventory(&at).unwrap(), before, "{case}: moved whole");
        }
        let from = original(&temp.path().join("edited"));
        std::fs::remove_dir_all(&gone).ok();
        copy_all(&from, &gone).unwrap();
        note(&from, &gone, &backups_dir, None);
        assert_eq!(moved_copy(&from, &backups_dir), Some(gone.clone()));
        std::fs::write(from.join("SKILL.md"), "edited after the kill").unwrap();
        assert_eq!(moved_copy(&from, &backups_dir), None, "a rest that changed");
        // Telling what it is reads no content; only deleting needs it whole.
        assert_eq!(noted_copy(&from, &backups_dir), Some(gone.clone()));
    }

    /// A special file in the rest of the original or in the copy is never
    /// read (a FIFO would block): the note does not check out.
    #[test]
    fn a_special_file_in_the_rest_or_the_copy_is_not_read() {
        let temp = tempfile::tempdir().unwrap();
        let backups_dir = temp.path().join("backups");
        for (i, side) in ["rest", "copy"].into_iter().enumerate() {
            let from = original(&temp.path().join(i.to_string()));
            let copy = backups_dir.join(format!("2026010{i}T000000Z/tool/skill"));
            std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
            copy_all(&from, &copy).unwrap();
            note(&from, &copy, &backups_dir, None);
            let pipe = match side {
                "rest" => from.join("pipe"),
                _ => copy.join("pipe"),
            };
            fifo(&pipe);

            assert_eq!(moved_copy(&from, &backups_dir), None, "{side}");
            assert!(from.join("SKILL.md").is_file(), "{side}");
        }
    }

    /// What is already at the partial copy's name, a file, a folder or a
    /// link, is never changed: the move stops before copying. A note left
    /// from before that does not check out is replaced.
    #[test]
    fn a_move_across_filesystems_leaves_whatever_is_in_its_way_alone() {
        let temp = tempfile::tempdir().unwrap();
        for (i, in_the_way) in ["file", "folder", "link"].iter().enumerate() {
            let dir = temp.path().join(i.to_string());
            let from = original(&dir);
            let mut backups = Backups::under(dir.join("backups"));
            let to = backups.fresh(Path::new("tool/skill")).unwrap();
            let taken = to.with_file_name(".skill.skiman-partial");
            match *in_the_way {
                "folder" => {
                    std::fs::create_dir(&taken).unwrap();
                    std::fs::write(taken.join("user-data"), "kept").unwrap();
                }
                "link" => std::os::unix::fs::symlink("kept", &taken).unwrap(),
                _ => std::fs::write(&taken, "kept").unwrap(),
            }
            let before = inventory(&taken).unwrap();

            let e = backups.move_across(&from, &to).unwrap_err();

            assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{in_the_way}");
            assert_eq!(inventory(&taken).unwrap(), before, "{in_the_way}");
            assert!(
                from.join("SKILL.md").is_file() && !to.exists(),
                "{in_the_way}"
            );
        }
        let dir = temp.path().join("stale");
        let from = original(&dir);
        note(&from, &dir.join("elsewhere"), &dir.join("backups"), None);
        let (_, moved, to) = across(&dir, &from);
        moved.unwrap();
        assert!(to.join("SKILL.md").is_file() && std::fs::symlink_metadata(&from).is_err());
    }

    /// The finish of a cut-short move renames the copy into this backup;
    /// when its folder then cannot be flushed, the error says where the
    /// copy is now, the note stays, and the backup counts as used.
    #[test]
    fn a_finish_whose_folder_cannot_be_flushed_says_where_the_copy_is() {
        let temp = tempfile::tempdir().unwrap();
        let entry = original(temp.path());
        let parent = temp.path().join("backups");
        let copy = parent.join("old/tool/skill");
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        copy_all(&entry, &copy).unwrap();
        let note = note(&entry, &copy, &parent, None);
        let before = inventory(&copy).unwrap();
        let mut backups = Backups::under(parent);
        // Write and search, no read: the rename goes in, the flush cannot open it.
        let folder = backups.dir().unwrap().join("tool");
        std::fs::DirBuilder::new()
            .mode(0o300)
            .create(&folder)
            .unwrap();

        let moved = backups.move_in(&entry, Path::new("tool/skill"));
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();

        let dest = folder.join("skill");
        let message = moved.unwrap_err().to_string();
        assert!(
            message.ends_with(&format!("; the complete copy is at {}", dest.display())),
            "{message}"
        );
        assert_eq!(inventory(&dest).unwrap(), before);
        assert!(std::fs::symlink_metadata(&entry).is_err());
        assert!(note.is_file(), "the note stays");
        assert!(backups.created().is_some());
    }

    #[test]
    fn the_backup_folder_is_created_when_the_first_thing_goes_in() {
        let temp = tempfile::tempdir().unwrap();
        let mut backups = Backups::under(temp.path().join("backups"));
        assert_eq!(backups.created(), None);
        assert!(!temp.path().join("backups").exists());

        let saved = backups
            .save(Path::new("repo/skiman.toml"), b"old\n")
            .unwrap();

        assert_eq!(std::fs::read(&saved).unwrap(), b"old\n");
        assert_eq!(backups.created(), saved.parent().unwrap().parent());
    }
}
