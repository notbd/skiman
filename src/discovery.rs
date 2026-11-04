//! Skill discovery (spec Source fetching): which folders of an upstream are
//! candidate skills.

use crate::tree;

/// The candidate folders among `files`, the file paths below the source
/// folder (relative to it). When the source folder itself holds `SKILL.md`,
/// it (`""`) is the only candidate. Otherwise every folder that directly
/// holds a `SKILL.md` is one, at any depth, skipping `.git` and not looking
/// inside another candidate. Sorted by path.
pub fn candidates(files: &[String]) -> Vec<String> {
    if files.iter().any(|f| f == "SKILL.md") {
        return vec![String::new()];
    }
    let mut folders: Vec<&str> = files
        .iter()
        .filter(|f| !f.split('/').any(|c| c.eq_ignore_ascii_case(".git")))
        .filter_map(|f| f.strip_suffix("/SKILL.md"))
        .collect();
    folders.sort_unstable();
    folders.dedup();
    let mut out: Vec<String> = Vec::new();
    for folder in folders {
        // Sorted, so a candidate's ancestors come before it.
        let inside_candidate = out
            .iter()
            .any(|c| folder.starts_with(c.as_str()) && folder[c.len()..].starts_with('/'));
        if !inside_candidate {
            out.push(folder.to_owned());
        }
    }
    out
}

/// `parent/child`, or `child` when `parent` is `""` (and `parent` when `child` is).
pub fn join(parent: &str, child: &str) -> String {
    if child.is_empty() {
        parent.to_owned()
    } else {
        tree::join(parent, child)
    }
}
