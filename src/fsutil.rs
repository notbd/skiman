//! Small filesystem helpers: writes that appear in one step, and lexical paths.

use std::io;
use std::path::{Component, Path, PathBuf};

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
