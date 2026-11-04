//! Reading an incoming skill's `SKILL.md` (from an upstream, or a foreign
//! skill for `adopt`) and the one rewrite skiman makes to it: inserting or
//! rewriting the single-line `name:` entry of the frontmatter, as a targeted
//! text edit (spec Language and packaging).

use saphyr::{LoadableYamlNode, MarkedYaml, Scalar, YamlData};

use crate::failure::Failure;
use crate::rules::frontmatter::split;

/// The frontmatter facts `add` needs from an incoming `SKILL.md`.
#[derive(Debug, Clone)]
pub struct SkillMd {
    /// What error messages call the file, e.g. `SKILL.md in https://...`.
    label: String,
    text: String,
    name: Option<NameEntry>,
}

#[derive(Debug, Clone)]
struct NameEntry {
    value: String,
    /// 0-based line of the file holding `name:`, when the whole entry is on
    /// that one line and starts the line; `None` otherwise.
    line: Option<usize>,
}

impl SkillMd {
    /// Refuses (exit 1, naming the file) a `SKILL.md` without frontmatter,
    /// without `description`, or whose `name` is not a string.
    pub fn parse(label: &str, content: &[u8]) -> Result<SkillMd, Failure> {
        SkillMd::read(label, content, true)
    }

    /// [`SkillMd::parse`] without the `description` requirement: `mv`
    /// rewrites only the `name` line and leaves the rest to the rules.
    pub fn parse_for_rename(label: &str, content: &[u8]) -> Result<SkillMd, Failure> {
        SkillMd::read(label, content, false)
    }

    fn read(label: &str, content: &[u8], require_description: bool) -> Result<SkillMd, Failure> {
        let refuse = |why: &str| Failure::refused(format!("{label} {why}"));
        let text = std::str::from_utf8(content).map_err(|_| refuse("is not valid UTF-8"))?;
        let yaml = split(text).ok_or_else(|| {
            refuse(
                "has no frontmatter: it must start on line 1 with `---` and end with a `---` line",
            )
        })?;
        let docs = MarkedYaml::load_from_str(yaml).map_err(|e| {
            refuse(&format!(
                "has invalid YAML in its frontmatter (line {}): {}",
                e.marker().line() + 1,
                e.info()
            ))
        })?;
        let Some(YamlData::Mapping(fields)) = docs.first().map(|d| &d.data) else {
            return Err(refuse("has a frontmatter that is not a mapping of fields"));
        };
        let field = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| matches!(&k.data, YamlData::Value(Scalar::String(s)) if s == key))
        };
        if require_description && field("description").is_none() {
            return Err(refuse("has no `description` in its frontmatter"));
        }
        let name = match field("name") {
            None => None,
            Some((key, value)) => {
                let YamlData::Value(Scalar::String(name)) = &value.data else {
                    return Err(refuse("has a `name` that is not a string"));
                };
                // YAML line n is file line n + 1 (1-based), so 0-based index n.
                let line = key.span.start.line();
                let single_line = key.span.start.col() == 0
                    && value.span.end.line() == line
                    && !name.contains('\n');
                Some(NameEntry {
                    value: name.to_string(),
                    line: single_line.then_some(line),
                })
            }
        };
        Ok(SkillMd {
            label: label.to_owned(),
            text: text.to_owned(),
            name,
        })
    }

    /// The frontmatter `name`, when there is one.
    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(|n| n.value.as_str())
    }

    /// The file with `name: <name>` in its frontmatter: the existing entry
    /// rewritten, or a new one inserted after the opening `---`. Refused
    /// (exit 1) when the existing entry is not a single-line scalar.
    pub fn with_name(&self, name: &str) -> Result<String, Failure> {
        let mut lines: Vec<String> = self.text.split_inclusive('\n').map(str::to_owned).collect();
        match &self.name {
            None => lines.insert(1, format!("name: {name}\n")),
            Some(NameEntry {
                line: Some(line), ..
            }) => {
                let ending = if lines[*line].ends_with("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                lines[*line] = format!("name: {name}{ending}");
            }
            Some(NameEntry { line: None, .. }) => {
                return Err(Failure::refused(format!(
                    "{} has a `name` entry that is not a single-line `name: <value>` line; skiman only rewrites a single-line `name:` entry, so it cannot set the name to `{name}`",
                    self.label
                )));
            }
        }
        Ok(lines.concat())
    }
}
