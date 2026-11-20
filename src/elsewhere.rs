//! The skills on this machine that skiman does not manage, for `skiman
//! overview`: what each present agent tool reads from places other managers
//! keep ([`SkillsElsewhere`] in [`TOOLS`]), and the skills in its tool
//! directory that skiman does not own. Read-only: nothing here is changed,
//! and a file another manager writes in a shape this module cannot read
//! becomes a note, not a failure.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::canon::Canon;
use crate::ownership::{Entry, Ownership};
use crate::paths::Paths;
use crate::project::root::written_form;
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

pub fn find(paths: &Paths, repo: &Canon) -> Found {
    let home = paths.home();
    let ownership = Ownership::new(paths, repo, None);
    let mut found = Collected::default();
    let present = || TOOLS.iter().filter(|tool| tool.is_present(home));
    for tool in present() {
        for place in tool.skills_elsewhere {
            match *place {
                SkillsElsewhere::Folders { dir, depth, owner } => {
                    for folder in skill_folders(&home.join(dir), depth) {
                        found.add(&folder, None, owner, tool.id, false);
                    }
                }
                SkillsElsewhere::ClaudePlugins { registry, settings } => {
                    let turned_off =
                        turned_off_plugins(home, &home.join(settings), &mut found.notes);
                    let registry = home.join(registry);
                    let skills =
                        plugin_skills(home, &registry, &turned_off, &ownership, &mut found.notes);
                    for skill in skills {
                        let name = skill.name.as_deref();
                        found.add(&skill.folder, name, &skill.owner, tool.id, false);
                    }
                }
            }
        }
        for what in tool.skills_not_listed {
            found.notes.push(format!(
                "`{}` also reads {what}; they are not listed.",
                tool.id
            ));
        }
    }
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
                found.add(&entry.path(), None, owner, tool.id, true);
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
    /// `name` is the skill's name; `None` takes the folder's.
    fn add(
        &mut self,
        path: &Path,
        name: Option<&str>,
        owner: &str,
        tool: &'static str,
        from_tool_dir: bool,
    ) {
        let Ok(location) = path.canonicalize() else {
            return;
        };
        let name = match name {
            Some(name) => name.to_owned(),
            None => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let skill = self
            .by_location
            .entry(location.clone())
            .or_insert_with(|| OutsideSkill {
                name,
                owner: owner.to_owned(),
                location,
                read_by: Vec::new(),
            });
        // Places are read before tool directories, so this only guards
        // against that order changing.
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

/// One skill of a Claude Code plugin.
struct PluginSkill {
    owner: String,
    folder: PathBuf,
    /// The plugin's name for a skill at the plugin's root, whose folder is
    /// named after the version; `None` takes the folder's name.
    name: Option<String>,
}

/// Every skill of every plugin in Claude Code's
/// plugin registry (`installed_plugins.json`, version 2). A missing registry
/// lists nothing; what cannot be read adds a note, and the rest is listed.
/// A plugin is listed when its `installPath`, as registered, is in the
/// registry's own folder (where Claude Code installs, even when a link on
/// the way, such as a moved cache, leads elsewhere), or when its realpath
/// is in a folder `ownership` leaves to other managers. Any other plugin
/// is a note: sync takes a link to one of its skills for one skiman did
/// not make, and so does the overview. A listed plugin's skills can still
/// lie outside `~/.claude/plugins` (a moved cache, a plugin installed in
/// link mode); a link in another tool's directory to one of them is in the
/// plugin's row here, while sync takes it for one skiman did not make.
fn plugin_skills(
    home: &Path,
    registry: &Path,
    turned_off: &[String],
    ownership: &Ownership,
    notes: &mut Vec<String>,
) -> Vec<PluginSkill> {
    let text = match std::fs::read(registry) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            notes.push(format!(
                "Cannot read {}: {e}; its plugins' skills are not listed.",
                written_form(home, registry)
            ));
            return Vec::new();
        }
    };
    let plugins = serde_json::from_slice::<Value>(&text)
        .ok()
        .filter(|v| v.get("version").and_then(Value::as_u64) == Some(2))
        .and_then(|v| v.get("plugins").and_then(Value::as_object).cloned());
    let Some(plugins) = plugins else {
        notes.push(format!(
            "{} is not in a format skiman reads; its plugins' skills are not listed.",
            written_form(home, registry)
        ));
        return Vec::new();
    };
    let mut skills = Vec::new();
    for (plugin, installs) in &plugins {
        let unreadable = || {
            format!(
                "The entry of plugin {plugin} in {} is not in a format skiman reads; its skills are not listed.",
                written_form(home, registry)
            )
        };
        let Some(installs) = installs.as_array() else {
            notes.push(unreadable());
            continue;
        };
        for install in installs {
            let Some(dir) = install.get("installPath").and_then(Value::as_str) else {
                notes.push(unreadable());
                continue;
            };
            let mut owner = format!("Claude Code plugin {plugin}");
            if let Some(version) = install.get("version").and_then(Value::as_str) {
                owner.push_str(&format!(" {version}"));
            }
            if let Some(scope) = install.get("scope").and_then(Value::as_str)
                && scope != "user"
            {
                // Claude Code 2.1 records the project of a `project` install.
                let project = install.get("projectPath").and_then(Value::as_str);
                match project.filter(|p| !p.is_empty()) {
                    Some(project) => owner.push_str(&format!(
                        " ({scope} scope: {})",
                        written_form(home, Path::new(project))
                    )),
                    None => owner.push_str(&format!(" ({scope} scope)")),
                }
            }
            let off = turned_off.contains(plugin);
            if off {
                owner.push_str(" (disabled)");
            }
            let root = Path::new(dir).canonicalize().ok();
            let in_registry = registry
                .parent()
                .is_some_and(|p| Path::new(dir).starts_with(p));
            // A missing folder gets its own note from `plugin_skill_folders`.
            if let Some(real) = root
                .as_ref()
                .filter(|r| !in_registry && !ownership.is_under_managed_dir(r))
            {
                let read = if off {
                    ""
                } else {
                    ", and Claude Code still reads them"
                };
                // The realpath: `installPath` can lead out through a link.
                notes.push(format!(
                    "{owner} is installed at {}, outside the folders skiman leaves to other managers; its skills are not listed{read}.",
                    written_form(home, real)
                ));
                continue;
            }
            let (folders, problems) = plugin_skill_folders(home, Path::new(dir));
            let plugin_name = plugin.split('@').next().unwrap_or(plugin);
            skills.extend(folders.into_iter().map(|folder| PluginSkill {
                name: (Some(&folder) == root.as_ref()).then(|| plugin_name.to_owned()),
                owner: owner.clone(),
                folder,
            }));
            notes.extend(problems.into_iter().map(|p| format!("{owner}: {p}.")));
        }
    }
    skills
}

/// The plugins `enabledPlugins` in Claude Code's settings file turns off
/// (`"<plugin>": false`); none when the file or the key is missing. A file
/// that cannot be read turns none off and adds a note.
fn turned_off_plugins(home: &Path, settings: &Path, notes: &mut Vec<String>) -> Vec<String> {
    let problem = match std::fs::read(settings) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => e.to_string(),
        Ok(text) => match serde_json::from_slice::<Value>(&text) {
            Err(e) => format!("it is not valid JSON: {e}"),
            Ok(Value::Object(settings)) => match settings.get("enabledPlugins") {
                None => return Vec::new(),
                Some(Value::Object(plugins)) => {
                    return plugins
                        .iter()
                        .filter(|(_, on)| on.as_bool() == Some(false))
                        .map(|(plugin, _)| plugin.clone())
                        .collect();
                }
                Some(_) => "its `enabledPlugins` is not an object".to_owned(),
            },
            Ok(_) => "it is not a JSON object".to_owned(),
        },
    };
    notes.push(format!(
        "Cannot tell which plugins {} turns off: {problem}; no plugin is marked (disabled).",
        written_form(home, settings)
    ));
    Vec::new()
}

/// The skill folders of the plugin installed at `dir`, as Claude Code loads
/// them: its `skills` folder, plus each path its `.claude-plugin/plugin.json`
/// lists under `skills` (a skill folder, or a folder of skill folders). A
/// path that resolves outside the plugin is left out. The plugin is `dir`,
/// and for one Claude Code installs in link mode also the folder its
/// `.claude-plugin-link` names, where the links in `dir` lead. Returns the
/// folders and what could not be read.
fn plugin_skill_folders(home: &Path, dir: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut problems = Vec::new();
    let Ok(root) = dir.canonicalize() else {
        problems.push(format!("its folder {} is missing", written_form(home, dir)));
        return (Vec::new(), problems);
    };
    let manifest = root.join(".claude-plugin/plugin.json");
    // The paths plugin.json lists, read after the `skills` folder.
    let mut listed = Vec::new();
    // Whether plugin.json has a `skills` key, of any value.
    let mut has_skills_key = false;
    match std::fs::read(&manifest) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => problems.push(format!(
            "cannot read {}: {e}",
            written_form(home, &manifest)
        )),
        Ok(text) => match serde_json::from_slice::<Value>(&text) {
            Err(e) => problems.push(format!(
                "{} is not valid JSON: {e}",
                written_form(home, &manifest)
            )),
            Ok(value) => {
                has_skills_key = value.get("skills").is_some();
                match value.get("skills") {
                    None => {}
                    Some(Value::String(path)) => listed.push(path.clone()),
                    Some(Value::Array(paths)) => {
                        for path in paths {
                            match path.as_str() {
                                Some(path) => listed.push(path.to_owned()),
                                None => problems.push(format!(
                                    "`skills` in {} lists `{path}`, which is not a path",
                                    written_form(home, &manifest)
                                )),
                            }
                        }
                    }
                    Some(_) => problems.push(format!(
                        "`skills` in {} is neither a path nor a list of paths",
                        written_form(home, &manifest)
                    )),
                }
            }
        },
    }
    // A plugin with a SKILL.md at its root, no `skills` folder and no
    // `skills` key is one skill.
    if !has_skills_key
        && root.join("SKILL.md").is_file()
        && root.join("skills").symlink_metadata().is_err()
    {
        return (vec![root], problems);
    }
    let linked_to = std::fs::read(root.join(".claude-plugin-link"))
        .ok()
        .and_then(|text| serde_json::from_slice::<Value>(&text).ok())
        .and_then(|link| Path::new(link.get("target")?.as_str()?).canonicalize().ok());
    let inside = |path: &Path| {
        path.canonicalize().ok().filter(|real| {
            real.starts_with(&root) || linked_to.as_ref().is_some_and(|t| real.starts_with(t))
        })
    };
    let mut folders = Vec::new();
    let skills_folder = std::iter::once(("skills".to_owned(), false));
    for (rel, from_manifest) in skills_folder.chain(listed.into_iter().map(|rel| (rel, true))) {
        let path = root.join(&rel);
        let Some(real) = inside(&path) else {
            if path.symlink_metadata().is_ok() {
                problems.push(format!("the skills path `{rel}` leads outside the plugin"));
            } else if from_manifest {
                problems.push(format!("the skills path `{rel}` does not exist"));
            }
            continue;
        };
        let candidates = if real.join("SKILL.md").is_file() {
            vec![real]
        } else {
            skill_folders(&real, 1)
        };
        for folder in candidates {
            match inside(&folder) {
                Some(real) if !folders.contains(&real) => folders.push(real),
                Some(_) => {}
                None => problems.push(format!(
                    "the skill folder {} leads outside the plugin",
                    written_form(home, &folder)
                )),
            }
        }
    }
    (folders, problems)
}
