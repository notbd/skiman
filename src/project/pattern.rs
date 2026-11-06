//! Which paths under a project root are project items, from the tool table's
//! `project_instruction_files` and `project_skill_dirs` (plan "Tool table").
//!
//! Patterns are `/`-separated; `**` matches any number of folders and `*`
//! any part of one name. A path is matched from its end: the earliest
//! position where a pattern matches the rest decides the item's anchor, the
//! project folder it belongs to (`proj/.claude/rules/sub/x.md` has anchor
//! `proj`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use crate::tools::TOOLS;

/// The two kinds of project item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// One instruction file.
    File,
    /// One skill folder (holding `SKILL.md` on disk).
    Skill,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Skill => "skill",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        match text {
            "file" => Some(Kind::File),
            "skill" => Some(Kind::Skill),
            _ => None,
        }
    }
}

/// The union of every tool's instruction-file patterns, deduplicated and sorted.
pub fn instruction_patterns() -> Vec<&'static str> {
    union(TOOLS.iter().flat_map(|t| t.project_instruction_files))
}

/// The union of every tool's project skill directories, deduplicated and sorted.
pub fn skill_dir_patterns() -> Vec<&'static str> {
    union(TOOLS.iter().flat_map(|t| t.project_skill_dirs))
}

fn union<'a>(patterns: impl Iterator<Item = &'a &'static str>) -> Vec<&'static str> {
    let set: BTreeSet<&'static str> = patterns.copied().collect();
    set.into_iter().collect()
}

/// The patterns split into components, computed once: discovery matches
/// every file of a root against them.
static INSTRUCTIONS: LazyLock<Vec<Vec<&'static str>>> =
    LazyLock::new(|| split(&instruction_patterns()));
static SKILL_DIRS: LazyLock<Vec<Vec<&'static str>>> =
    LazyLock::new(|| split(&skill_dir_patterns()));

fn split(patterns: &[&'static str]) -> Vec<Vec<&'static str>> {
    patterns.iter().map(|p| p.split('/').collect()).collect()
}

/// The item shape of the root-relative path `rel` (`/`-separated, no leading
/// `/`), or `None`: the kind and the anchor (root-relative, `""` for the root
/// itself). `skill_dirs` are the root's configured extra skill folders.
/// Shape only: whether a skill folder holds `SKILL.md` is not looked at.
pub fn shape(rel: &str, is_dir: bool, skill_dirs: &[String]) -> Option<(Kind, String)> {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    // Anything inside a skill folder's location is that skill's content,
    // never an item of its own (discovery never enters a skill folder).
    if (1..parts.len()).any(|len| skill_anchor(&parts[..len], skill_dirs).is_some()) {
        return None;
    }
    if is_dir {
        return skill_anchor(&parts, skill_dirs).map(|len| (Kind::Skill, parts[..len].join("/")));
    }
    (0..parts.len())
        .find(|&start| INSTRUCTIONS.iter().any(|p| matches(p, &parts[start..])))
        .map(|start| (Kind::File, parts[..start].join("/")))
}

/// When `parts` is a skill folder's location (a skill-dir pattern or a
/// configured skill folder, then one name): the length of its anchor.
fn skill_anchor(parts: &[&str], skill_dirs: &[String]) -> Option<usize> {
    let (_, parent) = parts.split_last()?;
    if parent.is_empty() {
        return None;
    }
    for dir in skill_dirs {
        let dir: Vec<&str> = dir.split('/').collect();
        if parent == dir.as_slice() {
            return Some(dir.len() - 1);
        }
    }
    (0..parent.len()).find(|&start| SKILL_DIRS.iter().any(|p| matches(p, &parent[start..])))
}

/// Whether `path` contains a tool's skill-dir pattern as consecutive
/// components anywhere (a `skill_dirs` entry may not).
pub fn contains_skill_dir_pattern(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    skill_dir_patterns().iter().any(|p| {
        let p: Vec<&str> = p.split('/').collect();
        (0..parts.len())
            .any(|start| (start + p.len()..=parts.len()).any(|end| matches(&p, &parts[start..end])))
    })
}

/// Whether the whole of `path` matches the whole of `pattern`.
fn matches(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| matches(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(name, tail)| component_matches(first, name) && matches(rest, tail)),
    }
}

/// `*` matches any run of characters within one name; everything else is literal.
fn component_matches(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((prefix, rest)) => {
            let Some(tail) = name.strip_prefix(prefix) else {
                return false;
            };
            (0..=tail.len())
                .filter(|&i| tail.is_char_boundary(i))
                .any(|i| component_matches(rest, &tail[i..]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(rel: &str) -> Option<String> {
        shape(rel, false, &[]).map(|(_, a)| a)
    }

    fn skill(rel: &str, dirs: &[&str]) -> Option<String> {
        let dirs: Vec<String> = dirs.iter().map(|d| d.to_string()).collect();
        shape(rel, true, &dirs).map(|(_, a)| a)
    }

    #[test]
    fn instruction_files_and_their_anchors() {
        assert_eq!(file("AGENTS.md").as_deref(), Some(""));
        assert_eq!(file("a/b/AGENTS.md").as_deref(), Some("a/b"));
        assert_eq!(file("p/.claude/CLAUDE.md").as_deref(), Some("p"));
        assert_eq!(file("p/.claude/rules/x.md").as_deref(), Some("p"));
        assert_eq!(file("p/.claude/rules/sub/deep/x.md").as_deref(), Some("p"));
        assert_eq!(file("p/AGENTS.override.md").as_deref(), Some("p"));
        assert_eq!(file("p/.claude/rules/x.txt"), None);
        assert_eq!(file("p/notes.md"), None);
        assert_eq!(file("p/agents.md"), None);
        assert_eq!(file("p/.claude/skills/s/AGENTS.md"), None);
        assert_eq!(file("p/.claude/skills/s/deep/CLAUDE.md"), None);
    }

    #[test]
    fn skill_folders_and_their_anchors() {
        assert_eq!(skill("p/.claude/skills/x", &[]).as_deref(), Some("p"));
        assert_eq!(skill(".agents/skills/x", &[]).as_deref(), Some(""));
        assert_eq!(skill("p/.agent/skills/x", &[]).as_deref(), Some("p"));
        assert_eq!(skill("p/.claude/skills", &[]), None);
        assert_eq!(skill("p/.gemini/skills/x", &[]), None);
        assert_eq!(
            skill("core/skills/x", &["core/skills"]).as_deref(),
            Some("core")
        );
        assert_eq!(skill("core/skills/x/y", &["core/skills"]), None);
        assert_eq!(skill("p/.claude/skills/s/.claude/skills/inner", &[]), None);
    }

    #[test]
    fn skill_dir_patterns_inside_paths() {
        assert!(contains_skill_dir_pattern("p/.claude/skills"));
        assert!(contains_skill_dir_pattern("p/.claude/skills/x/sub"));
        assert!(!contains_skill_dir_pattern("fivec-core/skills"));
    }

    #[test]
    fn wildcards() {
        assert!(component_matches("*.md", "a.md"));
        assert!(component_matches("*.md", ".md"));
        assert!(!component_matches("*.md", "a.mdx"));
        assert!(component_matches("a*b*c", "aXXbYc"));
    }
}
