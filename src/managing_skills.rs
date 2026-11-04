//! The `managing-skills` skill shipped in the binary. Sync materialises it,
//! with the skill repo path filled in, into `<repo>/.skiman/managing-skills/`.

use std::io;

use crate::fsutil;
use crate::repo::SkillRepo;

pub const NAME: &str = "managing-skills";

const TEMPLATE: &str = include_str!("../assets/managing-skills/SKILL.md");
const REPO_PLACEHOLDER: &str = "{{SKILL_REPO}}";

/// The `SKILL.md` content for this skill repo.
pub fn content(repo: &SkillRepo) -> String {
    TEMPLATE.replace(REPO_PLACEHOLDER, &repo.root().display().to_string())
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
