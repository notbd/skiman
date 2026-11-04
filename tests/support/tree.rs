use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Everything below `dir` (not following symlinks), keyed by relative path:
/// `dir`, `file <mode> <contents>` or `link -> <target>`. Compare two calls
/// to prove a command changed nothing. A missing `dir` gives an empty map.
pub fn tree(dir: impl AsRef<Path>) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    walk(dir.as_ref(), Path::new(""), &mut out);
    out
}

fn walk(root: &Path, rel: &Path, out: &mut BTreeMap<PathBuf, String>) {
    let Ok(entries) = std::fs::read_dir(root.join(rel)) else {
        return;
    };
    for entry in entries {
        let entry = entry.expect("read dir entry");
        let rel = rel.join(entry.file_name());
        let path = root.join(&rel);
        let meta = std::fs::symlink_metadata(&path).expect("stat entry");
        let value = if meta.is_symlink() {
            format!("link -> {}", std::fs::read_link(&path).unwrap().display())
        } else if meta.is_dir() {
            walk(root, &rel, out);
            "dir".to_owned()
        } else {
            let contents = std::fs::read(&path).expect("read file");
            format!(
                "file {:o} {}",
                meta.permissions().mode() & 0o777,
                String::from_utf8_lossy(&contents)
            )
        };
        out.insert(rel, value);
    }
}
