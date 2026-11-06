//! The `managing-skills` skill shipped in the binary. Sync materialises it,
//! with the skill repo path filled in, into `<repo>/.skiman/managing-skills/`.

use std::io;

use crate::fsutil;
use crate::repo::SkillRepo;
use crate::tools::TOOLS;

pub const NAME: &str = "managing-skills";

const TEMPLATE: &str = include_str!("../assets/managing-skills/SKILL.md");
const REPO_PLACEHOLDER: &str = "{{SKILL_REPO}}";

/// The `SKILL.md` content for this skill repo.
pub fn content(repo: &SkillRepo) -> String {
    let home_paths = |path: fn(&crate::tools::AgentTool) -> &'static str| {
        TOOLS
            .iter()
            .map(|tool| format!("`~/{}`", path(tool)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    TEMPLATE
        .replace(REPO_PLACEHOLDER, &repo.root().display().to_string())
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

/// Writes `.skiman/managing-skills/SKILL.md` only when its content differs.
/// Returns whether it wrote.
pub fn materialise(repo: &SkillRepo) -> io::Result<bool> {
    let dir = repo.managing_skills_dir();
    let file = dir.join("SKILL.md");
    let wanted = content(repo);
    if std::fs::read(&file).is_ok_and(|current| current == wanted.as_bytes()) {
        return Ok(false);
    }
    std::fs::create_dir_all(&dir)?;
    fsutil::write_atomic(&file, wanted.as_bytes())?;
    Ok(true)
}
