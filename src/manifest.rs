//! Reading `skiman.toml` (spec Data formats). Only what sync and the name
//! checks need is read here; the rules on its content are `skiman check`'s.

use std::collections::BTreeMap;

use toml_edit::{DocumentMut, Item, Table, Value, value};

use crate::failure::Failure;
use crate::repo::SkillRepo;

#[derive(Debug, Clone)]
pub struct Manifest {
    doc: DocumentMut,
}

/// The `[skills.<name>]` table of one skill, as far as distribution needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillEntry {
    /// `enabled`, default true.
    pub enabled: bool,
}

/// The keys of `[skills.<name>]` that describe a skill to a person.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillInfo {
    /// What the skill is for, in one line.
    pub summary: Option<String>,
    /// Where it came from before the skill repo held it: for an own skill,
    /// the only record of its source.
    pub origin: Option<String>,
    /// The day it was installed, `YYYY-MM-DD`.
    pub installed: Option<String>,
}

impl Manifest {
    pub fn load(repo: &SkillRepo) -> Result<Manifest, Failure> {
        let file = repo.manifest_file();
        let text = std::fs::read_to_string(&file)
            .map_err(|e| Failure::operational(format!("cannot read {}: {e}", file.display())))?;
        Manifest::parse(&text).map_err(|e| Failure::refused(format!("{}: {e}", file.display())))
    }

    pub fn parse(text: &str) -> Result<Manifest, String> {
        let doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
        Ok(Manifest { doc })
    }

    /// `[tools].enabled`, in file order; empty when absent. Non-string items are skipped.
    pub fn enabled_tools(&self) -> Vec<String> {
        self.doc
            .get("tools")
            .and_then(|t| t.get("enabled"))
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every `[skills.<name>]` table.
    pub fn skills(&self) -> BTreeMap<String, SkillEntry> {
        let Some(skills) = self.doc.get("skills").and_then(Item::as_table_like) else {
            return BTreeMap::new();
        };
        skills
            .iter()
            .map(|(name, item)| {
                let entry = SkillEntry {
                    enabled: item.get("enabled").and_then(Item::as_bool).unwrap_or(true),
                };
                (name.to_owned(), entry)
            })
            .collect()
    }

    /// What `[skills.<name>]` tells a person about the skill. Values of the
    /// wrong type read as absent; `skiman check` reports them.
    pub fn info(&self, name: &str) -> SkillInfo {
        let skill = self.doc.get("skills").and_then(|skills| skills.get(name));
        let field = |key| skill.and_then(|s| s.get(key));
        let text = |key| {
            field(key)
                .and_then(Item::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        SkillInfo {
            summary: text("summary"),
            origin: text("origin"),
            installed: field("installed")
                .and_then(Item::as_datetime)
                .filter(|d| d.time.is_none() && d.offset.is_none())
                .and_then(|d| d.date)
                .map(|d| d.to_string()),
        }
    }

    /// The `[skills.<name>.source]` table, when there is one. Missing or
    /// non-string `url` and `path` read as empty; a `ref_kind` other than
    /// `branch`, `tag` or `commit` reads as absent.
    pub fn source(&self, name: &str) -> Option<SourceRecord> {
        let source = self
            .doc
            .get("skills")?
            .get(name)?
            .get("source")?
            .as_table_like()?;
        let field = |key| source.get(key).and_then(Item::as_str).map(str::to_owned);
        Some(SourceRecord {
            url: field("url").unwrap_or_default(),
            path: field("path").unwrap_or_default(),
            git_ref: field("ref"),
            ref_kind: field("ref_kind").and_then(|kind| {
                ["branch", "tag", "commit"]
                    .into_iter()
                    .find(|known| *known == kind)
            }),
        })
    }
}

/// One `[projects.<id>]` table, as far as the project scope needs it.
/// Values of the wrong type read as absent; `skiman check` reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    pub id: String,
    /// `path`, as written: absolute or starting with `~/`.
    pub path: Option<String>,
    pub exclude: Vec<String>,
}

impl Manifest {
    /// Every `[projects.<id>]` table, in file order.
    pub fn projects(&self) -> Vec<ProjectEntry> {
        project_entries(self.doc.as_table())
    }
}

/// The `[projects.<id>]` tables of a parsed `skiman.toml` (`root` is its top
/// table), in file order. Used by [`Manifest`] and by the rules.
pub fn project_entries(root: &Table) -> Vec<ProjectEntry> {
    let Some(projects) = root.get("projects").and_then(Item::as_table_like) else {
        return Vec::new();
    };
    let strings = |item: &Item, key: &str| -> Vec<String> {
        item.get(key)
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    projects
        .iter()
        .map(|(id, item)| ProjectEntry {
            id: id.to_owned(),
            path: item.get("path").and_then(Item::as_str).map(str::to_owned),
            exclude: strings(item, "exclude"),
        })
        .collect()
}

/// One `[linked.<id>]` table (ADR 0010).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedEntry {
    pub id: String,
    /// `path` as written when the table is well formed; otherwise why it is not.
    pub path: Result<String, String>,
}

impl Manifest {
    /// Every `[linked.<id>]` table, sorted by id: between overlapping
    /// registrations and colliding names, the id that sorts first wins.
    pub fn linked(&self) -> Vec<LinkedEntry> {
        let Some(linked) = self.doc.get("linked").and_then(Item::as_table_like) else {
            return Vec::new();
        };
        let mut entries: Vec<LinkedEntry> = linked
            .iter()
            .map(|(id, item)| LinkedEntry {
                id: id.to_owned(),
                path: linked_path(id, item),
            })
            .collect();
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        entries
    }
}

/// The `path` of a well-formed `[linked.<id>]` table, or what is wrong with it.
fn linked_path(id: &str, item: &Item) -> Result<String, String> {
    let table = item
        .as_table_like()
        .ok_or_else(|| format!("[linked.{id}] is not a table"))?;
    if let Some(why) = crate::linked::invalid_id(id) {
        return Err(why);
    }
    if let Some((key, _)) = table.iter().find(|(key, _)| *key != "path") {
        return Err(format!("[linked.{id}] has the unknown key `{key}`"));
    }
    match table.get("path").and_then(Item::as_str) {
        Some(path) if crate::linked::is_path_form(path) => Ok(path.to_owned()),
        _ => Err(format!(
            "`path` in [linked.{id}] must be a string: an absolute path or one starting with `~/`"
        )),
    }
}

/// `text` (a `skiman.toml`) with a new `[projects.<id>]` table holding `path`.
/// Everything else keeps its bytes, except a `projects` table written inline,
/// which becomes a standard table (`subtable`).
pub fn with_project(text: &str, id: &str, path: &str) -> Result<String, String> {
    with_path_table(text, "projects", id, path)
}

/// `text` (a `skiman.toml`) without the `[projects.<id>]` table; the
/// `[projects]` table goes too when it is left empty and implicit.
pub fn without_project(text: &str, id: &str) -> Result<String, String> {
    without_table(text, "projects", id)
}

/// `text` (a `skiman.toml`) with a new `[linked.<id>]` table holding `path`,
/// as [`with_project`] adds a project root.
pub fn with_linked(text: &str, id: &str, path: &str) -> Result<String, String> {
    with_path_table(text, "linked", id, path)
}

/// `text` (a `skiman.toml`) without the `[linked.<id>]` table, as
/// [`without_project`] removes a project root.
pub fn without_linked(text: &str, id: &str) -> Result<String, String> {
    without_table(text, "linked", id)
}

/// `text` with a new `[<section>.<id>]` table holding `path`.
fn with_path_table(text: &str, section: &str, id: &str, path: &str) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    let mut table = Table::new();
    table.insert("path", value(path));
    table.decor_mut().set_prefix("\n");
    let parent = subtable(doc.as_table_mut(), section)
        .ok_or_else(|| format!("`{section}` is not a table"))?;
    parent.insert(id, Item::Table(table));
    Ok(doc.to_string())
}

/// `text` without the `[<section>.<id>]` table, and without `[<section>]`
/// when that leaves it empty.
fn without_table(text: &str, section: &str, id: &str) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    if let Some(parent) = doc.get_mut(section).and_then(Item::as_table_like_mut) {
        parent.remove(id);
        if parent.is_empty() {
            doc.remove(section);
        }
    }
    Ok(doc.to_string())
}

/// `text` (a `skiman.toml` or `skiman.lock`) without the `[skills.<name>]`
/// table and its subtables, and without the `[[secrets.allow]]` entries for
/// a path below `skills/<name>/` or `upstream/<name>/`. Everything else
/// keeps its bytes.
pub fn without_skill(text: &str, name: &str) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    if let Some(skills) = doc.get_mut("skills").and_then(Item::as_table_like_mut) {
        skills.remove(name);
    }
    if let Some(Item::ArrayOfTables(allows)) = doc
        .get_mut("secrets")
        .and_then(|secrets| secrets.get_mut("allow"))
    {
        let in_skill = |allow: &Table| {
            let path = allow.get("path").and_then(Item::as_str);
            path.is_some_and(|p| renamed_path(p, name, name).is_some())
        };
        allows.retain(|allow| !in_skill(allow));
    }
    Ok(doc.to_string())
}

/// `text` (a `skiman.toml`) with the skill `old` renamed to `new`: its
/// `[skills.<old>]` table (with `enabled` and `source`) and every
/// `[[secrets.allow]]` path under `skills/<old>/` or `upstream/<old>/`.
/// Everything else keeps its bytes.
pub fn with_skill_renamed(text: &str, old: &str, new: &str) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    rename_skill_table(&mut doc, old, new);
    if let Some(Item::ArrayOfTables(allows)) = doc
        .get_mut("secrets")
        .and_then(|secrets| secrets.get_mut("allow"))
    {
        for allow in allows.iter_mut() {
            let Some(path) = allow.get_mut("path").and_then(Item::as_value_mut) else {
                continue;
            };
            let Some(renamed) = path.as_str().and_then(|p| renamed_path(p, old, new)) else {
                continue;
            };
            let decor = path.decor().clone();
            *path = Value::from(renamed);
            *path.decor_mut() = decor;
        }
    }
    Ok(doc.to_string())
}

/// `path` with its `skills/<old>/` or `upstream/<old>/` prefix renamed.
fn renamed_path(path: &str, old: &str, new: &str) -> Option<String> {
    ["skills", "upstream"].into_iter().find_map(|root| {
        path.strip_prefix(&format!("{root}/{old}/"))
            .map(|rest| format!("{root}/{new}/{rest}"))
    })
}

/// Moves the `[skills.<old>]` item of `doc` (a `skiman.toml` or
/// `skiman.lock`) to `[skills.<new>]`. Its tables keep their place in the file.
pub(crate) fn rename_skill_table(doc: &mut DocumentMut, old: &str, new: &str) {
    if let Some(skills) = doc.get_mut("skills").and_then(Item::as_table_like_mut)
        && let Some(item) = skills.remove(old)
    {
        skills.insert(new, item);
    }
}

/// The text `init` writes into a new `skiman.toml`.
pub fn new_manifest_text(enabled_tools: &[&str]) -> String {
    let ids: Vec<String> = enabled_tools.iter().map(|id| format!("\"{id}\"")).collect();
    format!("[tools]\nenabled = [{}]\n", ids.join(", "))
}

/// The `[skills.<name>.source]` table of a third-party skill (spec Data formats).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRecord {
    pub url: String,
    pub path: String,
    pub git_ref: Option<String>,
    pub ref_kind: Option<&'static str>,
}

/// `text` (a `skiman.toml`) with a new `[skills.<name>.source]` table added.
/// Everything else keeps its bytes (comments, layout and key order), except
/// a `skills` or `skills.<name>` table written inline, which becomes a
/// standard table (`subtable`).
pub fn with_source(text: &str, name: &str, source: &SourceRecord) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    let mut table = Table::new();
    table.insert("url", value(&source.url));
    table.insert("path", value(&source.path));
    if let Some(git_ref) = &source.git_ref {
        table.insert("ref", value(git_ref));
    }
    if let Some(kind) = source.ref_kind {
        table.insert("ref_kind", value(kind));
    }
    table.decor_mut().set_prefix("\n");
    let skill = subtable(doc.as_table_mut(), "skills")
        .and_then(|skills| subtable(skills, name))
        .ok_or_else(|| format!("[skills.{name}] is not a table"))?;
    skill.insert("source", Item::Table(table));
    Ok(doc.to_string())
}

/// `text` (a `skiman.toml`) with `ref` and `ref_kind` of
/// `[skills.<name>.source]` set to `git_ref` and `ref_kind`, each removed
/// when `None`. A key that already holds its value keeps its bytes, so the
/// text is unchanged exactly when both already match.
pub fn with_source_ref(
    text: &str,
    name: &str,
    git_ref: Option<&str>,
    ref_kind: Option<&str>,
) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    let source = doc
        .get_mut("skills")
        .and_then(|skills| skills.get_mut(name))
        .and_then(|skill| skill.get_mut("source"))
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| format!("[skills.{name}.source] is not a table"))?;
    for (key, want) in [("ref", git_ref), ("ref_kind", ref_kind)] {
        match want {
            Some(want) if source.get(key).and_then(Item::as_str) != Some(want) => {
                source.insert(key, value(want));
            }
            Some(_) => {}
            None => {
                source.remove(key);
            }
        }
    }
    Ok(doc.to_string())
}

/// The table `key` of `parent` as a standard table that tables can join. A
/// missing table is added and one written inline converted (a comment on its
/// line is lost), both at the end; either gets a header only when it holds
/// values of its own. `None` when `key` holds a value of another kind, such
/// as a string or an array (the rules report it).
pub(crate) fn subtable<'a>(parent: &'a mut Table, key: &str) -> Option<&'a mut Table> {
    if let None | Some(Item::Value(Value::InlineTable(_))) = parent.get(key) {
        let mut table = parent
            .remove(key)
            .and_then(|item| item.into_table().ok())
            .unwrap_or_default();
        table.set_implicit(true);
        parent.insert(key, Item::Table(table));
    }
    parent.get_mut(key)?.as_table_mut()
}
