//! Linked folders (ADR 0010): folders of skills outside the canon,
//! registered in `skiman.toml` as `[linked.<id>]`, linked into the canon as
//! `linked/<id>` and from there into every tool directory. skiman never
//! writes inside one.

use crate::names;

/// The canon's folder of links to linked folders.
pub const LINKED_DIR: &str = "linked";

/// Why `id` cannot name a linked folder; `None` when it can. An id follows
/// the skill-name rule and is not reserved: a single-skill folder's skill
/// is named after it.
pub fn invalid_id(id: &str) -> Option<String> {
    if names::is_reserved(id) {
        Some(format!(
            "`{id}` is a reserved name and cannot be a linked folder id"
        ))
    } else if !names::is_valid(id) {
        Some(format!(
            "`{id}` is not a valid linked folder id: use lowercase letters, digits and single hyphens"
        ))
    } else {
        None
    }
}

/// Whether `path` has the form of a registered path: absolute or starting with `~/`.
pub fn is_path_form(path: &str) -> bool {
    path.starts_with('/') || path.starts_with("~/")
}
