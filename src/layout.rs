//! Where skills sit under `skills/` (ADRs 0008, 0009): `skills/<name>` for a
//! skill in no group, `skills/<group>/<name>` for one in a group, where a
//! group is a path of one or more group folders (`tools/pdf`). The folder is
//! the group; nothing else records it. Tool directories stay flat: a link is
//! named after its skill wherever the skill's folder sits.
//!
//! An entry under `skills/` or under a group folder is a group folder when it
//! is a folder without a `SKILL.md` that holds only folders; groups nest. Any
//! other entry there is read as a skill folder, so the rules report what is
//! wrong with it.

use std::collections::BTreeSet;
use std::io;

use crate::tree::{self, Kind, Tree};

pub const SKILLS: &str = "skills";

/// `skills/<name>`, or `skills/<group>/<name>` (`group` may be `a/b`).
pub fn skill_path(group: Option<&str>, name: &str) -> String {
    match group {
        Some(group) => format!("{SKILLS}/{group}/{name}"),
        None => format!("{SKILLS}/{name}"),
    }
}

/// One entry where a skill folder sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub name: String,
    /// The group folders it sits in, joined with `/`.
    pub group: Option<String>,
}

impl Placed {
    /// The skill folder, relative to the skill repo.
    pub fn path(&self) -> String {
        skill_path(self.group.as_deref(), &self.name)
    }
}

/// What `skills/` holds, read as skill folders and group folders.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    /// Every skill folder entry, in path order. A name in two groups is
    /// here twice (a rule violation).
    skills: Vec<Placed>,
    /// Every group folder as a path below `skills/` (`tools`, `tools/pdf`).
    groups: BTreeSet<String>,
}

impl Layout {
    pub fn read(tree: &dyn Tree) -> io::Result<Layout> {
        let mut layout = Layout::default();
        layout.read_dir(tree, None)?;
        Ok(layout)
    }

    /// Reads the entries of `skills/<group>` into the layout.
    fn read_dir(&mut self, tree: &dyn Tree, group: Option<&str>) -> io::Result<()> {
        let dir = group.map_or_else(|| SKILLS.to_owned(), |g| format!("{SKILLS}/{g}"));
        for entry in visible_children(tree, &dir)? {
            let name = tree::basename(&entry.path);
            if is_group(tree, &entry)? {
                let inner = group.map_or_else(|| name.to_owned(), |g| format!("{g}/{name}"));
                self.read_dir(tree, Some(&inner))?;
                self.groups.insert(inner);
            } else {
                self.skills.push(Placed {
                    name: name.to_owned(),
                    group: group.map(str::to_owned),
                });
            }
        }
        Ok(())
    }

    /// Every skill folder entry, in path order.
    pub fn skills(&self) -> &[Placed] {
        &self.skills
    }

    /// Every group folder, as a path below `skills/`.
    pub fn groups(&self) -> &BTreeSet<String> {
        &self.groups
    }

    /// A group folder named `name` (its last part), as a path below `skills/`.
    pub fn group_named(&self, name: &str) -> Option<&str> {
        self.groups
            .iter()
            .map(String::as_str)
            .find(|g| g.rsplit('/').next() == Some(name))
    }

    /// Where the skill `name` sits (its first entry when it sits in two places).
    pub fn find(&self, name: &str) -> Option<&Placed> {
        self.skills.iter().find(|placed| placed.name == name)
    }

    /// The skill folder of `name` relative to the skill repo; `skills/<name>`
    /// when there is none.
    pub fn path(&self, name: &str) -> String {
        self.find(name)
            .map_or_else(|| skill_path(None, name), Placed::path)
    }

    /// Every entry of `name`: more than one is a rule violation.
    pub fn all_of(&self, name: &str) -> Vec<&Placed> {
        self.skills.iter().filter(|p| p.name == name).collect()
    }

    pub fn names(&self) -> BTreeSet<String> {
        self.skills.iter().map(|p| p.name.clone()).collect()
    }

    /// `path` with the group left out: `skills/<group>/<name>/<rest>` reads
    /// `skills/<name>/<rest>`; any other path is returned as it is.
    pub fn without_group(&self, path: &str) -> String {
        let Some(rest) = path.strip_prefix("skills/") else {
            return path.to_owned();
        };
        let group = self
            .groups
            .iter()
            .filter(|g| {
                rest.strip_prefix(g.as_str())
                    .is_some_and(|r| r.starts_with('/'))
            })
            .max_by_key(|g| g.len());
        match group.map(|g| &rest[g.len() + 1..]) {
            Some(inner) if !inner.is_empty() => format!("{SKILLS}/{inner}"),
            _ => path.to_owned(),
        }
    }
}

/// The entries directly in `dir`, without ignored names.
fn visible_children(tree: &dyn Tree, dir: &str) -> io::Result<Vec<tree::Entry>> {
    Ok(tree
        .children(dir)?
        .into_iter()
        .filter(|e| !tree::is_ignored(&e.path))
        .collect())
}

/// A folder without a `SKILL.md` that holds only folders.
fn is_group(tree: &dyn Tree, entry: &tree::Entry) -> io::Result<bool> {
    if entry.kind != Kind::Dir || tree.kind(&format!("{}/SKILL.md", entry.path))?.is_some() {
        return Ok(false);
    }
    Ok(visible_children(tree, &entry.path)?
        .iter()
        .all(|child| child.kind == Kind::Dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::WorkingTree;

    fn layout(files: &[&str], dirs: &[&str]) -> Layout {
        let root = tempfile::tempdir().unwrap();
        for dir in dirs {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        for file in files {
            let path = root.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        Layout::read(&WorkingTree::new(root.path())).unwrap()
    }

    #[test]
    fn a_folder_without_skill_md_that_holds_folders_is_a_group() {
        let layout = layout(
            &[
                "skills/pdf/SKILL.md",
                "skills/pdf/scripts/run.sh",
                "skills/code/review/SKILL.md",
                "skills/code/ship/SKILL.md",
                "skills/code/.DS_Store",
            ],
            &[],
        );
        let paths: Vec<String> = layout.skills().iter().map(Placed::path).collect();
        assert_eq!(
            paths,
            ["skills/code/review", "skills/code/ship", "skills/pdf"]
        );
        assert_eq!(layout.groups(), &BTreeSet::from(["code".to_owned()]));
        assert_eq!(layout.path("ship"), "skills/code/ship");
        assert_eq!(layout.path("gone"), "skills/gone");
    }

    #[test]
    fn a_folder_without_skill_md_that_holds_a_file_is_a_broken_skill() {
        let layout = layout(&["skills/pdf/README.md", "skills/pdf/scripts/run.sh"], &[]);
        assert_eq!(layout.path("pdf"), "skills/pdf");
        assert!(layout.groups().is_empty());
    }

    #[test]
    fn an_empty_folder_is_an_empty_group() {
        let layout = layout(&[], &["skills/later"]);
        assert!(layout.skills().is_empty());
        assert!(layout.groups().contains("later"));
    }

    #[test]
    fn groups_nest() {
        let layout = layout(
            &[
                "skills/tools/pdf/fill/SKILL.md",
                "skills/tools/pdf/fill/scripts/run.sh",
                "skills/tools/ship/SKILL.md",
            ],
            &["skills/tools/later/soon"],
        );
        let paths: Vec<String> = layout.skills().iter().map(Placed::path).collect();
        assert_eq!(paths, ["skills/tools/pdf/fill", "skills/tools/ship"]);
        let groups: Vec<&str> = layout.groups().iter().map(String::as_str).collect();
        assert_eq!(
            groups,
            ["tools", "tools/later", "tools/later/soon", "tools/pdf"]
        );
        assert_eq!(layout.group_named("pdf"), Some("tools/pdf"));
        assert_eq!(layout.group_named("fill"), None);
        assert_eq!(
            layout.without_group("skills/tools/pdf/fill/scripts/run.sh"),
            "skills/fill/scripts/run.sh"
        );
    }

    #[test]
    fn without_group_drops_only_a_group() {
        let layout = layout(&["skills/code/review/SKILL.md", "skills/pdf/SKILL.md"], &[]);
        assert_eq!(
            layout.without_group("skills/code/review/a.md"),
            "skills/review/a.md"
        );
        assert_eq!(
            layout.without_group("skills/pdf/code/a.md"),
            "skills/pdf/code/a.md"
        );
        assert_eq!(
            layout.without_group("upstream/pdf/a.md"),
            "upstream/pdf/a.md"
        );
    }
}
