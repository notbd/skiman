//! Small filesystem helpers: writes that appear in one step, lexical paths,
//! and the check that a write's parent folders are real folders.

use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

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
/// applies), sets `permissions` on it when given, then renames it to `path`.
/// The file is created exclusively, so a symlink planted under its name is
/// never followed. On failure only that file is removed.
fn replace_atomic(
    path: &Path,
    contents: &[u8],
    mode: u32,
    permissions: Option<std::fs::Permissions>,
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
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Writes `contents` to a temporary file in the same directory, then renames
/// it to `path`. The new file gets the mode `std::fs::write` would give it.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    replace_atomic(path, contents, 0o666, None)
}

/// Writes `files` below `dir` (created even when `files` is empty), each
/// with mode `0o755` when executable, else `0o644`.
pub fn write_files(dir: &Path, files: &[SnapshotFile]) -> Result<(), Failure> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Failure::operational(format!("cannot create {}: {e}", dir.display())))?;
    for file in files {
        write_file(&dir.join(&file.path), file.executable, &file.content)?;
    }
    Ok(())
}

/// Writes one file of [`write_files`], creating its parent folders.
pub fn write_file(path: &Path, executable: bool, content: &[u8]) -> Result<(), Failure> {
    let io = |e: io::Error| Failure::operational(format!("cannot write {}: {e}", path.display()));
    std::fs::create_dir_all(path.parent().expect("file has a parent")).map_err(io)?;
    std::fs::write(path, content).map_err(io)?;
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(io)
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
    replace_atomic(path, contents, 0o600, Some(permissions))
}

/// Creates a symlink at `link` pointing to `target` under a temporary name in
/// the same directory, then renames it into place.
pub fn symlink_atomic(target: &Path, link: &Path) -> io::Result<()> {
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
    fn symlink_atomic_leaves_an_entry_under_its_temporary_name_alone() {
        let (_temp, dir) = root();
        let link = dir.join("notes");
        let victim = dir.join("victim");
        std::fs::write(&victim, "user original").unwrap();
        let planted = plant_link(&link, &victim);

        symlink_atomic(Path::new("/skills/notes"), &link).unwrap();

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
