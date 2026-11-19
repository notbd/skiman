//! The warning for a skill body that an enabled agent tool cuts without a
//! message (`skill_body_size_limit` in the tool table). A warning, never a
//! violation: the skill still loads, only the end of its body is lost.

use std::collections::BTreeSet;
use std::io;

use super::config::Records;
use super::{Violation, frontmatter, readable, rule};
use crate::layout::Layout;
use crate::tools::TOOLS;
use crate::tree::Tree;

/// The warning for `skill_md`, the bytes of a whole `SKILL.md`, while the
/// tools `enabled` are enabled; `None` when its body is within every
/// enabled tool's limit, or there is no frontmatter to split off (a
/// frontmatter violation of its own). Canon skills and linked skills share it.
pub fn warning(skill_md: &[u8], enabled: &[String]) -> Option<String> {
    let (limit, tools) = smallest_limit(enabled)?;
    let text = std::str::from_utf8(skill_md).ok()?;
    let size = frontmatter::body(text)?.trim().len() as u64;
    (size > limit).then(|| {
        format!(
            "the body is {size} bytes (without the frontmatter and surrounding whitespace); {tools} cuts it after {limit} bytes without a message"
        )
    })
}

/// The warning for each canon skill's `SKILL.md`.
pub(super) fn warnings(
    tree: &dyn Tree,
    records: &Records,
    out: &mut BTreeSet<Violation>,
) -> io::Result<()> {
    let Some(enabled) = records.enabled_tools() else {
        return Ok(());
    };
    if smallest_limit(&enabled).is_none() {
        return Ok(());
    }
    for placed in Layout::read(tree)?.skills() {
        let path = format!("{}/SKILL.md", placed.path());
        let Some(kind) = tree.kind(&path)? else {
            continue;
        };
        if !readable(kind) {
            continue;
        }
        if let Some(message) = warning(&tree.read(&path)?, &enabled) {
            out.insert(Violation::new(path, None, rule::SKILL_BODY_SIZE, message));
        }
    }
    Ok(())
}

/// The smallest skill body size limit among the `enabled` tools, with the
/// ids of the tools that have it.
fn smallest_limit(enabled: &[String]) -> Option<(u64, String)> {
    let tools = TOOLS
        .iter()
        .filter(|tool| enabled.iter().any(|id| id == tool.id));
    let limit = tools.clone().filter_map(|t| t.skill_body_size_limit).min()?;
    let ids: Vec<&str> = tools
        .filter(|t| t.skill_body_size_limit == Some(limit))
        .map(|t| t.id)
        .collect();
    Some((limit, ids.join(", ")))
}
