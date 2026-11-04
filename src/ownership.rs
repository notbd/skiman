//! Which tool-directory entries skiman owns (spec Ownership). Link targets
//! are compared lexically; the target need not exist.

use std::path::{Path, PathBuf};

use crate::fsutil::absolute_lexical;

/// The link target of the symlink at `entry`, made absolute against the
/// entry's directory and normalised lexically. `None` when `entry` is not a symlink.
pub fn link_target(entry: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(entry).ok()?;
    let dir = entry.parent().unwrap_or(Path::new("/"));
    Some(absolute_lexical(dir, &target))
}

/// Whether `entry` is a symlink whose target is exactly `wanted` (an absolute,
/// normalised path such as `<repo realpath>/skills/<n>`).
pub fn is_owned_link(entry: &Path, wanted: &Path) -> bool {
    link_target(entry).is_some_and(|target| target == wanted)
}
