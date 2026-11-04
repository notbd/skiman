//! Rules on the frontmatter of `skills/<name>/SKILL.md`: starts on line 1,
//! valid YAML without duplicate keys, `name` and `description` present and
//! well-formed. Unknown fields are allowed.

use std::collections::BTreeSet;

use saphyr::{LoadableYamlNode, MarkedYaml, Scalar, YamlData};

use super::{Violation, rule};
use crate::names;

pub const MAX_DESCRIPTION: usize = 1024;

/// `path` is `skills/<folder>/SKILL.md`.
pub fn check(path: &str, content: &[u8], out: &mut BTreeSet<Violation>) {
    let folder = path.split('/').nth(1).unwrap_or_default();
    let mut add = |line: Option<usize>, rule: &'static str, message: String| {
        out.insert(Violation::new(path, line, rule, message));
    };
    let Ok(text) = std::str::from_utf8(content) else {
        add(
            None,
            rule::FRONTMATTER,
            "SKILL.md is not valid UTF-8".to_owned(),
        );
        return;
    };
    let Some(yaml) = split(text) else {
        let message = if is_delimiter(text.lines().next().unwrap_or_default()) {
            "the frontmatter has no closing `---` line"
        } else {
            "the frontmatter must start on line 1 with `---`"
        };
        add(Some(1), rule::FRONTMATTER, message.to_owned());
        return;
    };
    // YAML line 1 is file line 2 (the opening `---` is line 1).
    let file_line = |yaml_line: usize| yaml_line + 1;
    let docs = match MarkedYaml::load_from_str(yaml) {
        Ok(docs) => docs,
        Err(e) => {
            let line = Some(file_line(e.marker().line()));
            if e.info() == "duplicated key in mapping" {
                add(
                    line,
                    rule::DUPLICATE_KEY,
                    "duplicate key in the frontmatter".to_owned(),
                );
            } else {
                add(
                    line,
                    rule::FRONTMATTER,
                    format!("the frontmatter is not valid YAML: {}", e.info()),
                );
            }
            return;
        }
    };
    let Some(YamlData::Mapping(fields)) = docs.first().map(|d| &d.data) else {
        add(
            Some(1),
            rule::FRONTMATTER,
            "the frontmatter must be a YAML mapping of fields".to_owned(),
        );
        return;
    };
    let field = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| matches!(&k.data, YamlData::Value(Scalar::String(s)) if s == key))
    };

    match field("name") {
        None => add(
            Some(1),
            rule::REQUIRED_FIELD,
            "the frontmatter has no `name`".to_owned(),
        ),
        Some((key, value)) => {
            let line = Some(file_line(key.span.start.line()));
            match &value.data {
                // When the name equals the folder name, an invalid or reserved
                // name is reported once, on the folder (rule skill-folder).
                YamlData::Value(Scalar::String(name)) if name != folder => {
                    if names::is_reserved(name) {
                        add(
                            line,
                            rule::NAME_RESERVED,
                            format!("`{name}` is a reserved skill name"),
                        );
                    } else if !names::is_valid(name) {
                        add(
                            line,
                            rule::NAME_FORMAT,
                            format!(
                                "`{name}` is not a valid skill name: use 1 to {} lowercase letters, digits and single hyphens between them",
                                names::MAX_LEN
                            ),
                        );
                    }
                    add(
                        line,
                        rule::NAME_FOLDER,
                        format!(
                            "`name` is `{name}` but the folder is `{folder}`; they must be equal"
                        ),
                    );
                }
                YamlData::Value(Scalar::String(_)) => {}
                _ => add(
                    line,
                    rule::REQUIRED_FIELD,
                    "`name` must be a string".to_owned(),
                ),
            }
        }
    }
    match field("description") {
        None => add(
            Some(1),
            rule::REQUIRED_FIELD,
            "the frontmatter has no `description`".to_owned(),
        ),
        Some((key, value)) => {
            let line = Some(file_line(key.span.start.line()));
            match &value.data {
                YamlData::Value(Scalar::String(description)) => {
                    let length = description.chars().count();
                    if !(1..=MAX_DESCRIPTION).contains(&length) {
                        add(
                            line,
                            rule::DESCRIPTION_LENGTH,
                            format!(
                                "`description` must be 1 to {MAX_DESCRIPTION} characters (it has {length})"
                            ),
                        );
                    }
                }
                _ => add(
                    line,
                    rule::REQUIRED_FIELD,
                    "`description` must be a string".to_owned(),
                ),
            }
        }
    }
}

fn is_delimiter(line: &str) -> bool {
    line.trim_end_matches('\r') == "---"
}

/// The YAML between the `---` on line 1 and the next `---` line.
fn split(text: &str) -> Option<&str> {
    let mut lines = text.split_inclusive('\n');
    if !is_delimiter(lines.next()?.trim_end_matches('\n')) {
        return None;
    }
    let start = text.find('\n')? + 1;
    let mut offset = start;
    for line in lines {
        if is_delimiter(line.trim_end_matches('\n')) {
            return Some(&text[start..offset]);
        }
        offset += line.len();
    }
    None
}
