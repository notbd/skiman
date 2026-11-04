//! Rules on `skiman.toml` (unknown keys are an error) and `skiman.lock`
//! (version 1 format), with line numbers from the parsed spans.

use std::collections::BTreeSet;
use std::io;

use toml_edit::{Document, Item, TableLike};

use super::{Scope, Violation, rule};
use crate::tree::Tree;

pub const MANIFEST: &str = "skiman.toml";
pub const LOCKFILE: &str = "skiman.lock";

const REF_KINDS: &[&str] = &["branch", "tag", "commit"];

/// One parsed config file. `doc` is `None` when the file is missing or does not parse.
pub struct ConfigFile {
    path: &'static str,
    text: String,
    doc: Option<Document<String>>,
    /// The syntax violation, or a missing `skiman.toml`.
    problem: Option<Violation>,
}

impl ConfigFile {
    fn read(tree: &dyn Tree, path: &'static str, required: bool) -> io::Result<ConfigFile> {
        let mut file = ConfigFile {
            path,
            text: String::new(),
            doc: None,
            problem: None,
        };
        if tree.kind(path)?.is_none() {
            if required {
                file.problem = Some(Violation::new(
                    path,
                    None,
                    rule::TOML_SYNTAX,
                    "the file does not exist",
                ));
            } else {
                file.doc = Some(Document::parse(String::new()).expect("empty TOML parses"));
            }
            return Ok(file);
        }
        let Ok(text) = String::from_utf8(tree.read(path)?) else {
            file.problem = Some(Violation::new(
                path,
                None,
                rule::TOML_SYNTAX,
                "not valid UTF-8",
            ));
            return Ok(file);
        };
        match Document::parse(text.clone()) {
            Ok(doc) => file.doc = Some(doc),
            Err(e) => {
                let line = e.span().map(|s| line_of(&text, s.start));
                let message = e.message().trim().to_owned();
                file.problem = Some(Violation::new(
                    path,
                    line,
                    rule::TOML_SYNTAX,
                    format!("not valid TOML: {message}"),
                ));
            }
        }
        file.text = text;
        Ok(file)
    }

    /// The `[skills]` table, when the file parsed and has one.
    fn skills(&self) -> Option<&dyn TableLike> {
        self.doc.as_ref()?.as_table().get("skills")?.as_table_like()
    }

    fn skill(&self, name: &str) -> Option<&Item> {
        self.skills()?.get(name)
    }

    fn skill_names(&self) -> BTreeSet<String> {
        self.skills()
            .map(|t| t.iter().map(|(k, _)| k.to_owned()).collect())
            .unwrap_or_default()
    }

    /// The line of `key` in `table`, falling back to `fallback`.
    fn key_line(&self, table: &dyn TableLike, key: &str, fallback: Option<usize>) -> Option<usize> {
        table
            .key(key)
            .and_then(|k| k.span())
            .or_else(|| table.get(key).and_then(Item::span))
            .map(|s| line_of(&self.text, s.start))
            .or(fallback)
    }

    fn violation(
        &self,
        line: Option<usize>,
        rule: &'static str,
        message: impl Into<String>,
    ) -> Violation {
        Violation::new(self.path, line, rule, message)
    }

    /// A table of this file, for reporting its keys.
    fn section<'a>(
        &'a self,
        table: &'a dyn TableLike,
        within: impl Into<String>,
        line: Option<usize>,
    ) -> Section<'a> {
        Section {
            file: self,
            table,
            within: within.into(),
            line,
        }
    }
}

/// One table of a config file: where it is and how messages name it.
struct Section<'a> {
    file: &'a ConfigFile,
    table: &'a dyn TableLike,
    /// How messages name the table, e.g. `[skills.pdf.source]`.
    within: String,
    /// Line of the table's header, for keys that are missing.
    line: Option<usize>,
}

impl Section<'_> {
    /// Reports every key not in `allowed`.
    fn unknown_keys(&self, allowed: &[&str], out: &mut BTreeSet<Violation>) {
        for (key, _) in self.table.iter() {
            if !allowed.contains(&key) {
                let line = self.file.key_line(self.table, key, self.line);
                out.insert(self.file.violation(
                    line,
                    rule::UNKNOWN_KEY,
                    format!(
                        "unknown key `{key}` in {} (allowed: {})",
                        self.within,
                        allowed.join(", ")
                    ),
                ));
            }
        }
    }

    /// Checks the value of `key` with `valid`; a missing key is reported only when `required`.
    fn value(
        &self,
        key: &str,
        required: bool,
        expected: &str,
        valid: fn(&Item) -> bool,
        out: &mut BTreeSet<Violation>,
    ) {
        match self.table.get(key) {
            None if required => {
                out.insert(self.file.violation(
                    self.line,
                    rule::TOML_VALUE,
                    format!("{} has no `{key}` ({expected})", self.within),
                ));
            }
            Some(item) if !valid(item) => {
                let line = self.file.key_line(self.table, key, self.line);
                out.insert(self.file.violation(
                    line,
                    rule::TOML_VALUE,
                    format!("`{key}` in {} must be {expected}", self.within),
                ));
            }
            _ => {}
        }
    }
}

/// Byte offset -> 1-based line.
fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset.min(text.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

fn is_string(item: &Item) -> bool {
    item.as_str().is_some()
}

/// `skiman.toml` and `skiman.lock` of a tree.
pub struct Records {
    pub manifest: ConfigFile,
    pub lock: ConfigFile,
}

impl Records {
    pub fn read(tree: &dyn Tree) -> io::Result<Records> {
        Ok(Records {
            manifest: ConfigFile::read(tree, MANIFEST, true)?,
            lock: ConfigFile::read(tree, LOCKFILE, false)?,
        })
    }

    /// Whether both files parsed, so Consistency can be decided.
    pub fn parsed(&self) -> bool {
        self.manifest.doc.is_some() && self.lock.doc.is_some()
    }

    pub fn manifest_skills(&self) -> BTreeSet<String> {
        self.manifest.skill_names()
    }

    pub fn lock_skills(&self) -> BTreeSet<String> {
        self.lock.skill_names()
    }

    /// The `[skills.<name>.source]` table, when there is one.
    pub fn source(&self, name: &str) -> Option<&dyn TableLike> {
        self.manifest.skill(name)?.get("source")?.as_table_like()
    }

    /// Line of the `[skills.<name>.source]` header (or of `[skills.<name>]`).
    pub fn source_line(&self, name: &str) -> Option<usize> {
        let skill = self.manifest.skill(name)?.as_table_like()?;
        let table_line = self
            .manifest
            .skills()
            .and_then(|s| self.manifest.key_line(s, name, None));
        self.manifest.key_line(skill, "source", table_line)
    }

    pub fn lock_entry_line(&self, name: &str) -> Option<usize> {
        self.lock
            .skills()
            .and_then(|s| self.lock.key_line(s, name, None))
    }

    /// The lock entry's `snapshot`, when it is a string.
    pub fn locked_snapshot(&self, name: &str) -> Option<&str> {
        self.lock.skill(name)?.get("snapshot")?.as_str()
    }

    /// Format rules on the parts of both files `scope` names.
    pub fn check(&self, scope: &Scope, out: &mut BTreeSet<Violation>) {
        let manifest_in_scope = scope.manifest || !scope.manifest_tables.is_empty();
        if manifest_in_scope && let Some(problem) = &self.manifest.problem {
            out.insert(problem.clone());
        }
        let lock_in_scope = scope.lock || !scope.lock_entries.is_empty();
        if lock_in_scope && let Some(problem) = &self.lock.problem {
            out.insert(problem.clone());
        }
        if let Some(doc) = &self.manifest.doc {
            if scope.manifest {
                self.check_manifest_top(doc, out);
            }
            for name in &scope.manifest_tables {
                self.check_manifest_skill(name, out);
            }
        }
        if let Some(doc) = &self.lock.doc {
            if scope.lock {
                self.check_lock_top(doc, out);
            }
            for name in &scope.lock_entries {
                self.check_lock_entry(name, out);
            }
        }
    }

    fn check_manifest_top(&self, doc: &Document<String>, out: &mut BTreeSet<Violation>) {
        let m = &self.manifest;
        let root = doc.as_table();
        m.section(root, "skiman.toml", Some(1))
            .unknown_keys(&["tools", "skills", "secrets"], out);
        for key in ["tools", "skills", "secrets"] {
            if root.get(key).is_some_and(|i| i.as_table_like().is_none()) {
                out.insert(m.violation(
                    m.key_line(root, key, Some(1)),
                    rule::TOML_VALUE,
                    format!("`{key}` must be a table"),
                ));
            }
        }
        if let Some(tools) = root.get("tools").and_then(Item::as_table_like) {
            let tools = m.section(tools, "[tools]", m.key_line(root, "tools", Some(1)));
            tools.unknown_keys(&["enabled"], out);
            let strings: fn(&Item) -> bool = |i| {
                i.as_array()
                    .is_some_and(|a| a.iter().all(|v| v.as_str().is_some()))
            };
            tools.value("enabled", false, "an array of tool ids", strings, out);
        }
        if let Some(secrets) = root.get("secrets").and_then(Item::as_table_like) {
            let line = m.key_line(root, "secrets", Some(1));
            m.section(secrets, "[secrets]", line)
                .unknown_keys(&["allow"], out);
            match secrets.get("allow") {
                Some(Item::ArrayOfTables(allow)) => {
                    for entry in allow.iter() {
                        let line = entry.span().map(|s| line_of(&m.text, s.start)).or(line);
                        let entry = m.section(entry, "[[secrets.allow]]", line);
                        entry.unknown_keys(&["path", "rule", "fingerprint"], out);
                        for key in ["path", "rule", "fingerprint"] {
                            entry.value(key, true, "a string", is_string, out);
                        }
                    }
                }
                Some(_) => {
                    let line = m.key_line(secrets, "allow", line);
                    out.insert(m.violation(
                        line,
                        rule::TOML_VALUE,
                        "`allow` must be written as [[secrets.allow]] tables",
                    ));
                }
                None => {}
            }
        }
    }

    fn check_manifest_skill(&self, name: &str, out: &mut BTreeSet<Violation>) {
        let m = &self.manifest;
        let (Some(skills), Some(item)) = (m.skills(), m.skill(name)) else {
            return;
        };
        let line = m.key_line(skills, name, Some(1));
        let Some(table) = item.as_table_like() else {
            out.insert(m.violation(
                line,
                rule::TOML_VALUE,
                format!("[skills.{name}] must be a table"),
            ));
            return;
        };
        let skill = m.section(table, format!("[skills.{name}]"), line);
        skill.unknown_keys(&["enabled", "source"], out);
        skill.value(
            "enabled",
            false,
            "true or false",
            |i| i.as_bool().is_some(),
            out,
        );
        let Some(source) = table.get("source") else {
            return;
        };
        let line = m.key_line(table, "source", line);
        let Some(source) = source.as_table_like() else {
            out.insert(m.violation(
                line,
                rule::TOML_VALUE,
                format!("[skills.{name}.source] must be a table"),
            ));
            return;
        };
        let source = m.section(source, format!("[skills.{name}.source]"), line);
        source.unknown_keys(&["url", "path", "ref", "ref_kind", "upstream_name"], out);
        source.value("url", true, "a string", is_string, out);
        source.value(
            "path",
            true,
            "a string (\"\" for the upstream's root)",
            is_string,
            out,
        );
        source.value("ref", false, "a string", is_string, out);
        source.value("upstream_name", false, "a string", is_string, out);
        let ref_kind: fn(&Item) -> bool = |i| i.as_str().is_some_and(|s| REF_KINDS.contains(&s));
        source.value(
            "ref_kind",
            false,
            "one of branch, tag, commit",
            ref_kind,
            out,
        );
    }

    fn check_lock_top(&self, doc: &Document<String>, out: &mut BTreeSet<Violation>) {
        let l = &self.lock;
        let root = doc.as_table();
        if root.is_empty() && l.text.trim().is_empty() {
            return;
        }
        l.section(root, "skiman.lock", Some(1))
            .unknown_keys(&["version", "skills"], out);
        match root.get("version") {
            Some(v) if v.as_integer() == Some(1) => {}
            Some(_) => {
                let line = l.key_line(root, "version", Some(1));
                out.insert(l.violation(
                    line,
                    rule::LOCK_VERSION,
                    "skiman.lock must have `version = 1`",
                ));
            }
            None => {
                out.insert(l.violation(
                    Some(1),
                    rule::LOCK_VERSION,
                    "skiman.lock has no `version = 1`",
                ));
            }
        }
        if root
            .get("skills")
            .is_some_and(|i| i.as_table_like().is_none())
        {
            out.insert(l.violation(
                l.key_line(root, "skills", Some(1)),
                rule::TOML_VALUE,
                "`skills` must be a table",
            ));
        }
    }

    fn check_lock_entry(&self, name: &str, out: &mut BTreeSet<Violation>) {
        let l = &self.lock;
        let (Some(skills), Some(item)) = (l.skills(), l.skill(name)) else {
            return;
        };
        let line = l.key_line(skills, name, Some(1));
        let Some(table) = item.as_table_like() else {
            out.insert(l.violation(
                line,
                rule::TOML_VALUE,
                format!("lock entry [skills.{name}] must be a table"),
            ));
            return;
        };
        let entry = l.section(table, format!("lock entry [skills.{name}]"), line);
        entry.unknown_keys(&["commit", "ref", "snapshot"], out);
        let commit: fn(&Item) -> bool = |i| {
            i.as_str()
                .is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        };
        entry.value("commit", false, "a 40-hex commit sha", commit, out);
        entry.value("ref", false, "a string", is_string, out);
        let snapshot: fn(&Item) -> bool = |i| {
            i.as_str()
                .and_then(|s| s.strip_prefix("sha256:"))
                .is_some_and(|h| {
                    h.len() == 64
                        && h.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
        };
        entry.value(
            "snapshot",
            true,
            "\"sha256:<64 lowercase hex>\"",
            snapshot,
            out,
        );
    }
}
