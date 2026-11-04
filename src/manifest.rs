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
            upstream_name: field("upstream_name"),
        })
    }
}

/// The TOML text of the `[skills.<name>]` table of `text` (a `skiman.toml`
/// or `skiman.lock`) with its subtables, as a backup fragment: what the user
/// pastes back in place of that skill's entry. Empty when there is none.
pub fn skill_fragment(text: &str, name: &str) -> Result<String, String> {
    let doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    let mut fragment = DocumentMut::new();
    if let Some(item) = doc.get("skills").and_then(|skills| skills.get(name)) {
        let skills = subtable(fragment.as_table_mut(), "skills").expect("new table");
        skills.insert(name, item.clone());
    }
    Ok(fragment.to_string().trim_start().to_owned())
}

/// `text` (a `skiman.toml` or `skiman.lock`) without the `[skills.<name>]`
/// table and its subtables. Everything else keeps its bytes.
pub fn without_skill(text: &str, name: &str) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    if let Some(skills) = doc.get_mut("skills").and_then(Item::as_table_like_mut) {
        skills.remove(name);
    }
    Ok(doc.to_string())
}

/// `text` (a `skiman.toml`) with the skill `old` renamed to `new`: its
/// `[skills.<old>]` table (with `enabled` and `source`), the source's
/// `upstream_name` (written when `new` differs from the upstream name,
/// removed when it equals it), and every `[[secrets.allow]]` path under
/// `skills/<old>/` or `upstream/<old>/`. Everything else keeps its bytes.
pub fn with_skill_renamed(text: &str, old: &str, new: &str) -> Result<String, String> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    rename_skill_table(&mut doc, old, new);
    if let Some(source) = doc
        .get_mut("skills")
        .and_then(|skills| skills.get_mut(new))
        .and_then(|skill| skill.get_mut("source"))
        .and_then(Item::as_table_like_mut)
    {
        let upstream_name = source
            .get("upstream_name")
            .and_then(Item::as_str)
            .unwrap_or(old)
            .to_owned();
        if upstream_name == new {
            source.remove("upstream_name");
        } else {
            source.insert("upstream_name", value(upstream_name));
        }
    }
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
    pub upstream_name: Option<String>,
}

/// `text` (a `skiman.toml`) with a new `[skills.<name>.source]` table added.
/// Everything else keeps its bytes: comments, layout and key order.
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
    if let Some(upstream_name) = &source.upstream_name {
        table.insert("upstream_name", value(upstream_name));
    }
    table.decor_mut().set_prefix("\n");
    let skill = subtable(doc.as_table_mut(), "skills")
        .and_then(|skills| subtable(skills, name))
        .ok_or_else(|| format!("[skills.{name}] is not a table"))?;
    skill.insert("source", Item::Table(table));
    Ok(doc.to_string())
}

/// The table `key` of `parent`, created as an implicit table (no header of
/// its own) when missing; `None` when `key` holds something else.
pub(crate) fn subtable<'a>(parent: &'a mut Table, key: &str) -> Option<&'a mut Table> {
    let item = parent.entry(key).or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    item.as_table_mut()
}
