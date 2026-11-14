//! Small filesystem helpers: writes that appear in one step, a rename that
//! never replaces, lexical paths, and the check that a write's parent
//! folders are real folders.

use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use rustix::io::Errno;

use crate::failure::Failure;
use crate::snapshot::SnapshotFile;

/// Creates a new entry next to `path` (same directory, so a rename stays on
/// one filesystem) with `create`, which fails with `AlreadyExists` when the
/// name is taken, and returns the name it used. Tries
/// `.{name}.skiman-tmp-{pid}`, then the same with `-1` to `-99` appended; an
/// entry already there is never touched. Starts with `.` so a leftover is
/// never taken for a skill.
fn create_temporary_sibling<T>(
    path: &Path,
    mut create: impl FnMut(&Path) -> io::Result<T>,
) -> io::Result<(PathBuf, T)> {
    let name = path
        .file_name()
        .expect("path has a file name")
        .to_string_lossy();
    let first = format!(".{name}.skiman-tmp-{}", std::process::id());
    for attempt in 0..=99 {
        let tmp = match attempt {
            0 => path.with_file_name(&first),
            n => path.with_file_name(format!("{first}-{n}")),
        };
        match create(&tmp) {
            Ok(created) => return Ok((tmp, created)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("the temporary names {first} to {first}-99 are all taken"),
    ))
}

/// Writes `contents` to a new temporary file created with `mode` (the umask
/// applies), sets `permissions` on it when given, then renames it to `path`;
/// `durable` flushes the file before the rename and the folder after. The
/// file is created exclusively, so a symlink planted under its name is never
/// followed. On failure only that file is removed.
fn replace_atomic(
    path: &Path,
    contents: &[u8],
    mode: u32,
    permissions: Option<std::fs::Permissions>,
    durable: bool,
) -> io::Result<()> {
    let (tmp, mut file) = create_temporary_sibling(path, |tmp| {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(tmp)
    })?;
    let result = file
        .write_all(contents)
        .and_then(|()| permissions.map_or(Ok(()), |p| file.set_permissions(p)))
        .and_then(|()| match durable {
            true => Ok(rustix::fs::fsync(&file)?),
            false => Ok(()),
        })
        .and_then(|()| std::fs::rename(&tmp, path))
        .and_then(|()| match durable {
            true => flush_path(path.parent().expect("a file has a parent")),
            false => Ok(()),
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Renames `from` to `to` without replacing anything at `to`: an entry there,
/// even one that appeared a moment ago, fails with `AlreadyExists`. Where
/// the filesystem has no such rename (exFAT, NFS), a symlink is made again
/// at `to` and a file hard-linked there, neither of which replaces anything;
/// anything else, or a file where there are no hard links either, fails
/// with `Unsupported` before anything moved. `CrossesDevices` when `from`
/// cannot be renamed to `to` ([`crossing`]).
pub fn place_new(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Err(e) if NO_FLAG.contains(&e) => place_without_noreplace(from, to),
        other => Ok(other?),
    }
}

/// What a filesystem answers a rename flag it does not have with.
const NO_FLAG: [Errno; 4] = [Errno::NOTSUP, Errno::OPNOTSUPP, Errno::INVAL, Errno::NOSYS];

/// Swaps the entries at `a` and `b` in one rename, so neither path is ever
/// empty: `b` gets what `a` held. Only the rename: an error means nothing
/// moved, and flushing is the caller's. `Unsupported` where the filesystem
/// has no such rename (HFS+, exFAT, NFS); `CrossesDevices` as for
/// [`place_new`].
pub fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    match renameat_with(CWD, a, CWD, b, RenameFlags::EXCHANGE) {
        Err(e) if NO_FLAG.contains(&e) => Err(io::Error::new(io::ErrorKind::Unsupported, e)),
        other => Ok(other?),
    }
}

/// [`place_new`] where the rename has no no-replace flag.
fn place_without_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    let placed = if meta.is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
    } else if meta.is_file() {
        std::fs::hard_link(from, to)
    } else {
        Err(io::ErrorKind::Unsupported.into())
    };
    match placed {
        Ok(()) => std::fs::remove_file(from),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(e) => Err(io::Error::new(io::ErrorKind::Unsupported, e)),
    }
}

/// [`place_new`] for new content, or for an entry moving to a name nothing
/// else uses: where the filesystem cannot place it without replacing, it
/// is renamed once nothing is found at `to`, as skiman always did there, so
/// what appears between the check and the rename is replaced: an empty
/// folder, or for a file where hard links fail too, any file or link.
pub fn place_new_or_rename(from: &Path, to: &Path) -> io::Result<()> {
    match place_new(from, to) {
        Err(e) if e.kind() == io::ErrorKind::Unsupported => match std::fs::symlink_metadata(to) {
            Ok(_) => Err(io::ErrorKind::AlreadyExists.into()),
            Err(_) => std::fs::rename(from, to),
        },
        other => other,
    }
}

/// Why `from` could not be renamed to `to` when the rename failed with
/// `CrossesDevices`; nothing moved. On Linux a folder can also come from a
/// lower layer of a container image (overlayfs), which cannot be renamed.
pub fn crossing(from: &Path, to: &Path) -> String {
    let folder = std::fs::symlink_metadata(from).is_ok_and(|m| m.is_dir());
    let overlay = match cfg!(target_os = "linux") && folder {
        true => ", or it is a folder from a lower layer of a container image (overlayfs)",
        false => "",
    };
    format!(
        "cannot rename {} to {}: they are on different filesystems{overlay}; nothing was moved",
        from.display(),
        to.display()
    )
}

/// [`place_new_or_rename`] for content that has to last (ADR 0005), then
/// `to`'s folder flushed. Whoever wrote `from` flushed it; an original is
/// not skiman's to flush, and `from`'s old folder is not flushed either.
pub fn publish(from: &Path, to: &Path) -> io::Result<()> {
    place_new_or_rename(from, to)?;
    flush_path(to.parent().expect("a destination has a parent"))
}

/// Writes `contents` to a new file at `path`, which must not exist: under a
/// temporary name first, flushed, then renamed in without replacing, and
/// the folder flushed. A reader sees all of it or nothing.
pub fn write_new(path: &Path, contents: &[u8]) -> io::Result<()> {
    let (tmp, mut file) = create_temporary_sibling(path, |tmp| File::create_new(tmp))?;
    let written = file
        .write_all(contents)
        .and_then(|()| Ok(rustix::fs::fsync(&file)?));
    written.and_then(|()| publish(&tmp, path)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The devices this run flushed to, each with the last path flushed there.
static FLUSHED: Mutex<Vec<(u64, PathBuf)>> = Mutex::new(Vec::new());

/// Asks the system to write what the file or folder at `path` holds to the
/// disk (`fsync`), and remembers its device: on Apple platforms `fsync`
/// reaches only the drive's own cache, and [`drain`] takes it the rest of
/// the way. A folder some filesystems refuse to fsync (`EBADF`, `EINVAL`)
/// counts as flushed, as in PostgreSQL's `fsync_fname_ext`.
pub fn flush_path(path: &Path) -> io::Result<()> {
    let file = File::open(path)?;
    let meta = file.metadata()?;
    match rustix::fs::fsync(&file) {
        Err(e) if meta.is_dir() && [Errno::BADF, Errno::INVAL].contains(&e) => {}
        done => done?,
    }
    let device = meta.dev();
    let mut flushed = FLUSHED.lock().unwrap_or_else(PoisonError::into_inner);
    flushed.retain(|(d, _)| *d != device);
    flushed.push((device, path.to_owned()));
    Ok(())
}

/// Puts everything flushed on the device that holds `path` on the disk,
/// the drive's cache included: `F_FULLFSYNC` on Apple platforms, falling
/// back to `fsync` where a filesystem refuses it, as SQLite does; elsewhere
/// `fsync` already waits for the device.
pub fn barrier(path: &Path) -> io::Result<()> {
    let file = File::open(path)?;
    #[cfg(target_vendor = "apple")]
    if rustix::fs::fcntl_fullfsync(&file).is_ok() {
        return Ok(());
    }
    Ok(rustix::fs::fsync(&file)?)
}

/// [`barrier`] once on every device this run flushed to (on a path that
/// still exists there); skiman calls it before it exits.
pub fn drain() -> io::Result<()> {
    let flushed = std::mem::take(&mut *FLUSHED.lock().unwrap_or_else(PoisonError::into_inner));
    for (_, path) in flushed {
        let existing = path.ancestors().find(|p| p.exists()).unwrap_or(&path);
        let failed = |e: io::Error| format!("cannot flush {}: {e}", existing.display());
        barrier(existing).map_err(|e| io::Error::new(e.kind(), failed(e)))?;
    }
    Ok(())
}

/// Flushes every file and folder at or below `root`, links not followed.
pub fn flush_tree(root: &Path) -> io::Result<()> {
    for (path, meta) in entries(root)? {
        if !meta.is_symlink() {
            flush_path(&path)?;
        }
    }
    Ok(())
}

/// Removes each folder at or below `dir` that is empty once the ones below it
/// are, deepest first; files and links stay (but on macOS volumes without
/// native xattrs, exFAT, `rmdir` also takes `._*` files). With a folder it
/// cannot read, nothing is removed. Empty folders are no content: tree
/// hashes leave them out.
pub fn remove_empty_dirs(dir: &Path) {
    for (path, meta) in entries(dir).unwrap_or_default().into_iter().rev() {
        if meta.is_dir() {
            let _ = std::fs::remove_dir(path);
        }
    }
}

/// Every entry at or below `path`, parents before what they hold, never
/// following symlinks.
pub fn entries(path: &Path) -> io::Result<Vec<(PathBuf, std::fs::Metadata)>> {
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

/// The text of the file at `path`; `None` when there is none.
pub fn read_text(path: &Path) -> Result<Option<String>, Failure> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Failure::operational(format!(
            "cannot read {}: {e}",
            path.display()
        ))),
    }
}

/// Writes `contents` to a temporary file in the same directory, flushed,
/// then renames it to `path` and flushes the folder. The new file gets the
/// mode `std::fs::write` would give it.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    replace_atomic(path, contents, 0o666, None, true)
}

/// [`write_atomic`] without the flushes, for content skiman derives again
/// when it is lost (sync's own files).
pub fn write_derived(path: &Path, contents: &[u8]) -> io::Result<()> {
    replace_atomic(path, contents, 0o666, None, false)
}

/// Writes `files` below `dir` (created even when `files` is empty), each
/// with mode `0o755` when executable, else `0o644`, each flushed, then
/// the folders they are in, for [`publish`].
pub fn write_files(dir: &Path, files: &[SnapshotFile]) -> Result<(), Failure> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Failure::operational(format!("cannot create {}: {e}", dir.display())))?;
    for file in files {
        write_file(&dir.join(&file.path), file.executable, &file.content)?;
    }
    let folders = entries(dir).and_then(|all| {
        let mut folders = all.into_iter().filter(|(_, meta)| meta.is_dir());
        folders.try_for_each(|(folder, _)| flush_path(&folder))
    });
    folders.map_err(|e| Failure::operational(format!("cannot flush {}: {e}", dir.display())))
}

/// Writes one file of [`write_files`], creating its parent folders, and
/// flushes it, mode included.
pub fn write_file(path: &Path, executable: bool, content: &[u8]) -> Result<(), Failure> {
    let io = |e: io::Error| Failure::operational(format!("cannot write {}: {e}", path.display()));
    std::fs::create_dir_all(path.parent().expect("file has a parent")).map_err(io)?;
    let mode = if executable { 0o755 } else { 0o644 };
    let mut file = File::create(path).map_err(io)?;
    file.write_all(content)
        .and_then(|()| file.set_permissions(std::fs::Permissions::from_mode(mode)))
        .and_then(|()| Ok(rustix::fs::fsync(&file)?))
        .map_err(io)
}

/// Like [`write_atomic`], with mode `0o755`, so `path` is never seen without its execute bits.
pub fn write_atomic_executable(path: &Path, contents: &[u8]) -> io::Result<()> {
    rewrite_atomic(path, contents, std::fs::Permissions::from_mode(0o755))
}

/// Like [`write_atomic`], but the new file gets `permissions` before the rename,
/// so rewriting a file keeps its mode.
pub fn rewrite_atomic(
    path: &Path,
    contents: &[u8],
    permissions: std::fs::Permissions,
) -> io::Result<()> {
    replace_atomic(path, contents, 0o600, Some(permissions), true)
}

/// Points skiman's own link `link` at `target`: creates the new link under a
/// temporary name in the same directory, then renames it over `link`. The
/// rename replaces whatever is at `link`, so it is only for a link skiman owns.
pub fn retarget_symlink(target: &Path, link: &Path) -> io::Result<()> {
    let (tmp, ()) = create_temporary_sibling(link, |tmp| std::os::unix::fs::symlink(target, tmp))?;
    std::fs::rename(&tmp, link).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Makes `path` absolute against `base` and normalises it lexically: `.` is
/// dropped and `..` removes the previous component. Nothing is resolved on disk.
pub fn absolute_lexical(base: &Path, path: &Path) -> PathBuf {
    let joined = base.join(path);
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The realpath of the deepest existing ancestor of `path`, joined with the rest.
pub fn realpath_as_far_as_exists(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(real) = current.canonicalize() {
            return rest.iter().rev().fold(real, |p: PathBuf, c| p.join(c));
        }
        match (current.file_name().map(|n| n.to_owned()), current.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                current = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Why `dest` cannot be written: `dest` is not below `inside`, or a component
/// between `inside` and `dest`'s parent exists but is not a real folder or
/// cannot be read. `None` when the way is clear: only a component that does
/// not exist (`NotFound`) counts as not created yet, and the write creates it.
///
/// `inside` is a trusted root the caller has already canonicalized; it is not
/// checked itself. `dest` is compared with it lexically, so build `dest` by
/// joining a relative path without `..` onto `inside`.
pub fn broken_chain(inside: &Path, dest: &Path) -> Option<String> {
    let Some(rel) = dest.parent().and_then(|p| p.strip_prefix(inside).ok()) else {
        return Some(format!(
            "{} is outside {}",
            dest.display(),
            inside.display()
        ));
    };
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
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => return Some(format!("cannot read {}: {e}", dir.display())),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the filesystem has no no-replace rename (exFAT, NFS), what
    /// appeared at the destination after any check is still never replaced:
    /// the fallback is called as `place_new` calls it after `ENOTSUP`.
    #[test]
    fn without_a_no_replace_rename_a_link_or_file_never_replaces_and_a_folder_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        std::os::unix::fs::symlink("target", dir.join("link")).unwrap();
        std::fs::write(dir.join("file"), "new").unwrap();
        std::fs::create_dir(dir.join("folder")).unwrap();
        for (from, appeared) in [("link", "appeared-file"), ("file", "appeared-link")] {
            match appeared {
                "appeared-file" => std::fs::write(dir.join(appeared), "kept").unwrap(),
                _ => std::os::unix::fs::symlink("kept", dir.join(appeared)).unwrap(),
            }
            let e = place_without_noreplace(&dir.join(from), &dir.join(appeared)).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{from}");
            assert!(
                std::fs::symlink_metadata(dir.join(from)).is_ok(),
                "{from} stays"
            );
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("appeared-file")).unwrap(),
            "kept"
        );
        assert_eq!(
            std::fs::read_link(dir.join("appeared-link")).unwrap(),
            Path::new("kept")
        );
        let e = place_without_noreplace(&dir.join("folder"), &dir.join("free")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(dir.join("folder").is_dir() && !dir.join("free").exists());

        place_without_noreplace(&dir.join("link"), &dir.join("link2")).unwrap();
        place_without_noreplace(&dir.join("file"), &dir.join("file2")).unwrap();
        assert_eq!(
            std::fs::read_link(dir.join("link2")).unwrap(),
            Path::new("target")
        );
        assert_eq!(std::fs::read_to_string(dir.join("file2")).unwrap(), "new");
        assert!(std::fs::symlink_metadata(dir.join("link")).is_err() && !dir.join("file").exists());
    }

    #[test]
    fn drain_reaches_a_flushed_folder_that_is_gone_through_its_parent() {
        let temp = tempfile::tempdir().unwrap();
        let gone = temp.path().join("staged");
        std::fs::create_dir(&gone).unwrap();
        flush_path(&gone).unwrap();
        std::fs::remove_dir(&gone).unwrap();

        // Other tests flush on their own threads: only the result is checked.
        drain().unwrap();
    }

    #[test]
    fn place_new_never_replaces_an_entry_and_moves_into_a_free_name() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        std::fs::write(dir.join("new"), "new").unwrap();
        std::fs::create_dir(dir.join("folder")).unwrap();
        std::fs::write(dir.join("taken"), "kept").unwrap();
        std::fs::create_dir(dir.join("empty")).unwrap();
        for (from, to) in [("new", "taken"), ("folder", "empty"), ("folder", "taken")] {
            let e = place_new(&dir.join(from), &dir.join(to)).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{from} to {to}");
        }
        assert_eq!(std::fs::read_to_string(dir.join("taken")).unwrap(), "kept");
        assert!(dir.join("empty").is_dir());

        place_new(&dir.join("new"), &dir.join("placed")).unwrap();
        place_new(&dir.join("folder"), &dir.join("moved")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("placed")).unwrap(), "new");
        assert!(dir.join("moved").is_dir() && !dir.join("folder").exists());
    }

    fn root() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        (temp, root)
    }

    /// Plants a symlink to `victim` under the first temporary name for `path`.
    fn plant_link(path: &Path, victim: &Path) -> PathBuf {
        let name = path.file_name().unwrap().to_string_lossy();
        let planted = path.with_file_name(format!(".{name}.skiman-tmp-{}", std::process::id()));
        std::os::unix::fs::symlink(victim, &planted).unwrap();
        planted
    }

    type Writer = fn(&Path, &[u8]) -> io::Result<()>;

    #[test]
    fn a_writer_neither_follows_nor_removes_a_symlink_planted_under_its_temporary_name() {
        let writers: [(&str, Writer); 3] = [
            ("write_atomic", write_atomic),
            ("rewrite_atomic", |path, contents| {
                rewrite_atomic(path, contents, std::fs::Permissions::from_mode(0o640))
            }),
            ("write_atomic_executable", write_atomic_executable),
        ];
        for (writer, write) in writers {
            let (_temp, dir) = root();
            let victim = dir.join("victim");
            std::fs::write(&victim, "user original").unwrap();
            let path = dir.join("SKILL.md");
            let planted = plant_link(&path, &victim);

            write(&path, b"new").unwrap();

            assert_eq!(
                std::fs::read_to_string(&victim).unwrap(),
                "user original",
                "{writer} wrote through the planted link"
            );
            assert_eq!(std::fs::read_link(&planted).unwrap(), victim, "{writer}");
            assert!(
                std::fs::symlink_metadata(&path).unwrap().is_file(),
                "{writer}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"new", "{writer}");
        }
    }

    #[test]
    fn retarget_symlink_leaves_an_entry_under_its_temporary_name_alone() {
        let (_temp, dir) = root();
        let link = dir.join("notes");
        let victim = dir.join("victim");
        std::fs::write(&victim, "user original").unwrap();
        let planted = plant_link(&link, &victim);

        retarget_symlink(Path::new("/skills/notes"), &link).unwrap();

        assert_eq!(std::fs::read_link(&planted).unwrap(), victim);
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            Path::new("/skills/notes")
        );
    }

    #[test]
    fn a_writer_fails_without_touching_anything_when_every_temporary_name_is_taken() {
        let (_temp, dir) = root();
        let path = dir.join("config.toml");
        let first = dir.join(format!(".config.toml.skiman-tmp-{}", std::process::id()));
        let mut taken = vec![first.clone()];
        taken.extend((1..=99).map(|i| PathBuf::from(format!("{}-{i}", first.display()))));
        for name in &taken {
            std::fs::write(name, "taken").unwrap();
        }

        let e = write_atomic(&path, b"new").unwrap_err();

        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{e}");
        assert!(std::fs::symlink_metadata(&path).is_err());
        for name in &taken {
            assert_eq!(std::fs::read_to_string(name).unwrap(), "taken");
        }
    }

    #[test]
    fn write_atomic_creates_a_file_with_the_mode_fs_write_gives() {
        let (_temp, dir) = root();
        std::fs::write(dir.join("plain"), "x").unwrap();
        write_atomic(&dir.join("atomic"), b"x").unwrap();

        let mode = |name| {
            std::fs::metadata(dir.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777
        };
        assert_eq!(mode("atomic"), mode("plain"));
    }

    #[test]
    fn rewrite_atomic_gives_the_file_the_permissions_it_is_passed() {
        let (_temp, dir) = root();
        let path = dir.join("settings.json");
        for mode in [0o600, 0o644, 0o755, 0o666] {
            rewrite_atomic(&path, b"{}", std::fs::Permissions::from_mode(mode)).unwrap();

            let meta = std::fs::metadata(&path).unwrap();
            assert_eq!(meta.permissions().mode() & 0o7777, mode, "{mode:o}");
        }
    }

    #[test]
    fn a_dest_outside_inside_is_refused() {
        let (_temp, root) = root();
        let inside = root.join("repo");
        std::fs::create_dir(&inside).unwrap();
        let dest = root.join("elsewhere/AGENTS.md");

        assert_eq!(
            broken_chain(&inside, &dest),
            Some(format!(
                "{} is outside {}",
                dest.display(),
                inside.display()
            ))
        );
    }

    #[test]
    fn only_a_missing_component_counts_as_not_created_yet() {
        let (_temp, inside) = root();
        assert_eq!(
            broken_chain(&inside, &inside.join("new/dir/AGENTS.md")),
            None
        );

        // A name longer than the filesystem allows cannot be read: refused.
        let dest = inside.join("a".repeat(300)).join("AGENTS.md");
        let why = broken_chain(&inside, &dest).expect("refused");
        assert!(why.starts_with("cannot read "), "{why}");
    }
}
