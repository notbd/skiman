//! The skills on this machine that skiman does not manage, for `skiman
//! overview`: what each present agent tool reads from places other managers
//! keep ([`SkillsElsewhere`] in [`TOOLS`]), and the skills in its tool
//! directory that skiman does not own. Read-only: nothing here is changed,
//! and a file another manager writes in a shape this module cannot read
//! becomes a note, not a failure.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::manifest::Manifest;
use crate::ownership::{Entry, Ownership};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::tools::{SkillsElsewhere, TOOLS};

/// One skill folder another manager keeps, or nobody known does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideSkill {
    /// The folder's name.
    pub name: String,
    /// Who keeps it, such as `claude.ai sync`.
    pub owner: String,
    /// Realpath of the skill folder: one row per folder, however many
    /// tools reach it.
    pub location: PathBuf,
    /// Ids of the agent tools that read it, in [`TOOLS`] order.
    pub read_by: Vec<&'static str>,
}

/// What [`find`] found.
#[derive(Debug, Default)]
pub struct Found {
    /// Sorted by owner, then name.
    pub skills: Vec<OutsideSkill>,
    /// What could not be listed, one sentence each.
    pub notes: Vec<String>,
}

/// The owner shown for a skill in a tool directory that skiman does not own.
const FOREIGN_LINK: &str = "unknown: a link skiman did not make";
const FOREIGN_FOLDER: &str = "unknown: a folder skiman did not make";
/// The owner shown for an entry the ownership rules give to another manager
/// when no [`SkillsElsewhere`] place names it.
const ANOTHER_MANAGER: &str = "another manager";

pub fn find(paths: &Paths, repo: &SkillRepo, manifest: &Manifest) -> Found {
    let home = paths.home();
    let mut found = Collected::default();
    let present = || TOOLS.iter().filter(|tool| tool.is_present(home));
    for tool in present() {
        for place in tool.skills_elsewhere {
            match *place {
                SkillsElsewhere::Folders { dir, depth, owner } => {
                    for folder in skill_folders(&home.join(dir), depth) {
                        found.add(&folder, owner, tool.id, false);
                    }
                }
                SkillsElsewhere::ClaudePlugins { registry } => {
                    for (owner, folder) in plugin_skills(&home.join(registry), &mut found.notes) {
                        found.add(&folder, &owner, tool.id, false);
                    }
                }
            }
        }
        if tool.builtin_skills_off_disk {
            found.notes.push(format!(
                "`{}` also has built-in skills that are not stored on disk, so they are not listed.",
                tool.id
            ));
        }
    }
    let ownership = Ownership::new(paths, repo, manifest, None);
    for tool in present() {
        let Ok(real_dir) = tool.tool_dir(home).canonicalize() else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&real_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            let owner = match ownership.classify(&real_dir, &entry.file_name(), &meta) {
                Entry::Foreign if meta.is_symlink() => FOREIGN_LINK,
                Entry::Foreign => FOREIGN_FOLDER,
                Entry::ManagedElsewhere => ANOTHER_MANAGER,
                Entry::Owned(_) | Entry::NotASkill | Entry::Moved(_) => continue,
            };
            // A managed-elsewhere folder that is not itself a skill (claude.ai's
            // `synced`) is listed through its place, if at all.
            if entry.path().join("SKILL.md").is_file() {
                found.add(&entry.path(), owner, tool.id, true);
            }
        }
    }
    found.finish()
}

#[derive(Default)]
struct Collected {
    by_location: BTreeMap<PathBuf, OutsideSkill>,
    notes: Vec<String>,
}

impl Collected {
    /// Records the skill folder at `path` (any link resolved) as read by
    /// `tool`. A folder seen before keeps its owner, except that a place's
    /// owner replaces the vaguer one of a tool-directory entry.
    fn add(&mut self, path: &Path, owner: &str, tool: &'static str, from_tool_dir: bool) {
        let Ok(location) = path.canonicalize() else {
            return;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let skill = self
            .by_location
            .entry(location.clone())
            .or_insert_with(|| OutsideSkill {
                name,
                owner: owner.to_owned(),
                location,
                read_by: Vec::new(),
            });
        if !from_tool_dir && skill.owner == ANOTHER_MANAGER {
            skill.owner = owner.to_owned();
        }
        if !skill.read_by.contains(&tool) {
            skill.read_by.push(tool);
        }
    }

    fn finish(self) -> Found {
        let order = |id: &&str| TOOLS.iter().position(|tool| tool.id == *id);
        let mut skills: Vec<OutsideSkill> = self.by_location.into_values().collect();
        for skill in &mut skills {
            skill.read_by.sort_by_key(order);
        }
        skills.sort_by(|a, b| {
            (&a.owner, &a.name, &a.location).cmp(&(&b.owner, &b.name, &b.location))
        });
        Found {
            skills,
            notes: self.notes,
        }
    }
}

/// Folders exactly `depth` levels below `dir` that hold a `SKILL.md`;
/// names starting with `.` are skipped. Sorted.
fn skill_folders(dir: &Path, depth: u8) -> Vec<PathBuf> {
    let mut level = vec![dir.to_path_buf()];
    for _ in 0..depth {
        level = level
            .iter()
            .flat_map(|d| sorted_children(d))
            .filter(|p| p.is_dir())
            .collect();
    }
    level.retain(|p| p.join("SKILL.md").is_file());
    level
}

/// The entries of `dir` whose names do not start with `.`, sorted; none
/// when it cannot be read.
fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| !e.file_name().as_encoded_bytes().starts_with(b"."))
        .map(|e| e.path())
        .collect();
    children.sort();
    children
}

/// `(owner, skill folder)` for every skill of every plugin in Claude Code's
/// plugin registry (`installed_plugins.json`, version 2). A missing registry
/// lists nothing; one that cannot be read adds a note.
fn plugin_skills(registry: &Path, notes: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    let text = match std::fs::read(registry) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            notes.push(format!(
                "Cannot read {}: {e}; its plugins' skills are not listed.",
                registry.display()
            ));
            return Vec::new();
        }
    };
    let plugins = serde_json::from_slice::<Value>(&text)
        .ok()
        .and_then(|v| v.get("plugins").and_then(Value::as_object).cloned());
    let Some(plugins) = plugins else {
        notes.push(format!(
            "{} is not in a format skiman reads; its plugins' skills are not listed.",
            registry.display()
        ));
        return Vec::new();
    };
    let mut skills = Vec::new();
    for (plugin, installs) in &plugins {
        for install in installs.as_array().into_iter().flatten() {
            let Some(dir) = install.get("installPath").and_then(Value::as_str) else {
                continue;
            };
            let owner = match install.get("version").and_then(Value::as_str) {
                Some(version) => format!("Claude Code plugin {plugin} {version}"),
                None => format!("Claude Code plugin {plugin}"),
            };
            match plugin_skill_folders(Path::new(dir)) {
                Ok(folders) => skills.extend(folders.into_iter().map(|f| (owner.clone(), f))),
                Err(problem) => {
                    notes.push(format!("{owner}: {problem}; its skills are not listed."))
                }
            }
        }
    }
    skills
}

/// The skill folders of the plugin installed at `dir`: each path its
/// `.claude-plugin/plugin.json` lists under `skills` (a skill folder, or a
/// folder of skill folders), or the `skills` folder when it lists none.
fn plugin_skill_folders(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let manifest = dir.join(".claude-plugin/plugin.json");
    let listed: Vec<String> = match std::fs::read(&manifest) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec!["skills".to_owned()],
        Err(e) => return Err(format!("cannot read {}: {e}", manifest.display())),
        Ok(text) => {
            let value: Value = serde_json::from_slice(&text)
                .map_err(|e| format!("{} is not valid JSON: {e}", manifest.display()))?;
            match value.get("skills") {
                None => vec!["skills".to_owned()],
                Some(Value::String(path)) => vec![path.clone()],
                Some(Value::Array(paths)) => paths
                    .iter()
                    .filter_map(|p| p.as_str().map(str::to_owned))
                    .collect(),
                Some(_) => {
                    return Err(format!(
                        "`skills` in {} is neither a path nor a list of paths",
                        manifest.display()
                    ));
                }
            }
        }
    };
    let mut folders = Vec::new();
    for rel in listed {
        let rel = Path::new(&rel);
        // Only paths inside the plugin: a plugin cannot point this listing elsewhere.
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| c == std::path::Component::ParentDir)
        {
            continue;
        }
        let path = dir.join(rel);
        if path.join("SKILL.md").is_file() {
            folders.push(path);
        } else {
            folders.extend(skill_folders(&path, 1));
        }
    }
    Ok(folders)
}
