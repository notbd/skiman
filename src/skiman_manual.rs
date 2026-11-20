//! skiman's manual, the `skiman-manual` skill shipped in the binary. Sync
//! materialises it, with the canon path filled in, into
//! `<repo>/.skiman/skiman-manual/`.

use std::io;

use crate::canon::Canon;
use crate::fsutil;
use crate::tools::TOOLS;

pub const NAME: &str = "skiman-manual";

const TEMPLATE: &str = include_str!("../assets/skiman-manual/SKILL.md");
const CANON_PLACEHOLDER: &str = "{{CANON}}";

/// The `SKILL.md` content for this canon.
pub fn content(repo: &Canon) -> String {
    let home_paths = |path: fn(&crate::tools::AgentTool) -> &'static str| {
        TOOLS
            .iter()
            .map(|tool| format!("`~/{}`", path(tool)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    TEMPLATE
        .replace(CANON_PLACEHOLDER, &repo.root().display().to_string())
        .replace("{{TOOL_DIRECTORIES}}", &home_paths(|t| t.tool_dir))
        .replace("{{INSTRUCTION_FILES}}", &home_paths(|t| t.instruction_file))
        .replace(
            "{{PROJECT_INSTRUCTION_FILES}}",
            &code_list(&crate::project::pattern::instruction_patterns()),
        )
        .replace(
            "{{PROJECT_SKILL_DIRS}}",
            &code_list(&crate::project::pattern::skill_dir_patterns()),
        )
}

/// `a`, `b` and `c` written as inline code, joined with commas.
fn code_list(items: &[&str]) -> String {
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Writes `.skiman/skiman-manual/SKILL.md` only when its content differs,
/// and removes the manual's folder from before its rename (derived content,
/// so no backup; a link there is removed, not followed). Returns whether it
/// wrote. Fails without writing when a folder on the way is not a real
/// folder: a link there could lead the write out of the repo.
pub fn materialise(repo: &Canon) -> io::Result<bool> {
    let dir = repo.manual_dir();
    let file = dir.join("SKILL.md");
    if let Some(why) = fsutil::broken_chain(repo.root(), &file) {
        return Err(io::Error::other(why));
    }
    let retired = repo.retired_manual_dir();
    let removed = match std::fs::symlink_metadata(&retired) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&retired),
        Ok(_) => std::fs::remove_file(&retired),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    removed.map_err(|e| io::Error::other(format!("cannot remove {}: {e}", retired.display())))?;
    let wanted = content(repo);
    if std::fs::read(&file).is_ok_and(|current| current == wanted.as_bytes()) {
        return Ok(false);
    }
    std::fs::create_dir_all(&dir)?;
    fsutil::write_derived(&file, wanted.as_bytes())?;
    Ok(true)
}
