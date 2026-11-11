//! Small filesystem helpers: writes that appear in one step, lexical paths,
//! and the check that a write's parent folders are real folders.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use crate::failure::Failure;
use crate::snapshot::SnapshotFile;

/// A temporary name next to `path` (same directory, so a rename stays on one
/// filesystem). Starts with `.` so a leftover is never taken for a skill.
fn temporary_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .expect("path has a file name")
        .to_string_lossy();
    path.with_file_name(format!(".{name}.skiman-tmp-{}", std::process::id()))
}

/// Writes `contents` to a temporary file in the same directory, then renames it to `path`.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let tmp = temporary_sibling(path);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
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
    let tmp = temporary_sibling(path);
    let result = std::fs::write(&tmp, contents)
        .and_then(|()| std::fs::set_permissions(&tmp, permissions))
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Creates a symlink at `link` pointing to `target` under a temporary name in
/// the same directory, then renames it into place.
pub fn symlink_atomic(target: &Path, link: &Path) -> io::Result<()> {
    let tmp = temporary_sibling(link);
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
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
