//! Backups under machine state (spec Data formats, Machine state):
//! `backups/<timestamp>/` with `repo/<path relative to the skill repo>` and
//! `<tool id>/<path relative to the tool directory>` inside, created when
//! first needed. A backup destination is always fresh. A move across
//! filesystems copies, verifies, then deletes the original.

use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::paths::Paths;
use crate::tree::{self, IGNORED_NAMES};

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

/// One command's backup folder, `backups/<timestamp>/`, created when the
/// first thing goes in, so a command that backs nothing up leaves no folder.
#[derive(Debug)]
pub struct Backups {
    parent: PathBuf,
    dir: Option<PathBuf>,
    /// Whether anything went in.
    used: bool,
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
        std::fs::write(&dest, content)?;
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

    /// Moves `original` to `<backup>/<rel>` (a symlink as a symlink). Across
    /// filesystems: copy, verify, then delete the original; an original
    /// holding a special file is refused and left in place.
    pub fn move_in(&mut self, original: &Path, rel: &Path) -> io::Result<PathBuf> {
        let dest = self.fresh(rel)?;
        match std::fs::rename(original, &dest) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                move_across_filesystems(original, &dest)?;
            }
            Err(e) => return Err(e),
        }
        self.used = true;
        Ok(dest)
    }
}

fn move_across_filesystems(original: &Path, dest: &Path) -> io::Result<()> {
    if let Some(special) = special_files(original)?.first() {
        return Err(io::Error::other(format!(
            "{} is a special file (FIFO, socket or device); move it away by hand",
            special.display()
        )));
    }
    let copied = copy_all(original, dest).and_then(|()| verify(original, dest));
    if let Err(e) = copied {
        let _ = remove(dest);
        return Err(e);
    }
    remove(original)
}

fn remove(path: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Every entry at or below `path`, never following symlinks.
fn entries(path: &Path) -> io::Result<Vec<(PathBuf, std::fs::Metadata)>> {
    let mut out = vec![(path.to_path_buf(), std::fs::symlink_metadata(path)?)];
    let mut i = 0;
    while i < out.len() {
        if out[i].1.is_dir() {
            for entry in std::fs::read_dir(&out[i].0)? {
                let entry = entry?;
                let meta = std::fs::symlink_metadata(entry.path())?;
                out.push((entry.path(), meta));
            }
        }
        i += 1;
    }
    Ok(out)
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

/// The tree hash (for a single file, its sha256; for a symlink, its target),
/// every file with an ignored name ([`IGNORED_NAMES`]), and every symlink's
/// path and target are the same at `to`.
fn verify(from: &Path, to: &Path) -> io::Result<()> {
    let mismatch = |what: &str| {
        io::Error::other(format!(
            "the copy at {} differs from {} ({what})",
            to.display(),
            from.display()
        ))
    };
    let top = std::fs::symlink_metadata(from)?;
    if top.is_symlink() {
        let same = std::fs::symlink_metadata(to)?.is_symlink()
            && std::fs::read_link(to)? == std::fs::read_link(from)?;
        if !same {
            return Err(mismatch("symlink target"));
        }
        return Ok(());
    }
    if top.is_file() {
        if Sha256::digest(std::fs::read(from)?) != Sha256::digest(std::fs::read(to)?) {
            return Err(mismatch("content"));
        }
        return Ok(());
    }
    if tree::tree_hash_of_dir(from)? != tree::tree_hash_of_dir(to)? {
        return Err(mismatch("tree hash"));
    }
    for (entry, meta) in entries(from)? {
        let target = copy_path(from, to, &entry);
        let name = entry.file_name().unwrap_or_default().to_string_lossy();
        if meta.is_symlink() {
            let same = std::fs::symlink_metadata(&target).is_ok_and(|m| m.is_symlink())
                && std::fs::read_link(&target)? == std::fs::read_link(&entry)?;
            if !same {
                return Err(mismatch(&format!("symlink {}", entry.display())));
            }
        } else if IGNORED_NAMES.contains(&name.as_ref()) && !target.is_file() {
            return Err(mismatch(&format!("missing {}", entry.display())));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
