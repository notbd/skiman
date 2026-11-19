//! A linked skill's summary (ADR 0011): `metadata.summary` in its own
//! `SKILL.md`, which skiman never writes; without one, the first sentence of
//! its description stands in, marked as a stand-in wherever it is shown.

use saphyr::{AnnotatedMapping, LoadableYamlNode, MarkedYaml, Scalar, YamlData};

use crate::rules::frontmatter::split;

/// What a linked skill's frontmatter says about it for a person.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct About {
    /// `metadata.summary`, when it is a string with more than whitespace.
    pub summary: Option<String>,
    /// `description`, when it is a string.
    pub description: Option<String>,
}

impl About {
    /// Reads the frontmatter of the `SKILL.md` text `text`; `None` when it
    /// does not parse as a YAML mapping.
    pub fn read(text: &str) -> Option<About> {
        let docs = MarkedYaml::load_from_str(split(text)?).ok()?;
        let YamlData::Mapping(fields) = &docs.first()?.data else {
            return None;
        };
        let summary = match field(fields, "metadata").map(|m| &m.data) {
            Some(YamlData::Mapping(metadata)) => string(metadata, "summary"),
            _ => None,
        };
        Some(About {
            summary: summary.filter(|s| !s.trim().is_empty()),
            description: string(fields, "description"),
        })
    }
}

/// The value of the field `key` of `map`.
fn field<'m, 'a>(
    map: &'m AnnotatedMapping<'a, MarkedYaml<'a>>,
    key: &str,
) -> Option<&'m MarkedYaml<'a>> {
    map.iter()
        .find(|(k, _)| matches!(&k.data, YamlData::Value(Scalar::String(s)) if s == key))
        .map(|(_, value)| value)
}

/// The field `key` of `map`, when it is a string.
fn string(map: &AnnotatedMapping<MarkedYaml>, key: &str) -> Option<String> {
    match &field(map, key)?.data {
        YamlData::Value(Scalar::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// The stand-in for a missing summary (ADR 0011): the first sentence of
/// `description`, up to the first `.`, `!` or `?` followed by whitespace or
/// the end, the first `。`, `！` or `？`, or the first line break, whichever
/// comes first; cut to 120 characters with `…`.
pub fn stand_in(description: &str) -> String {
    let chars: Vec<char> = description.trim().chars().collect();
    let mut end = chars.len();
    for (i, c) in chars.iter().enumerate() {
        let next_blank = chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        if matches!(c, '。' | '！' | '？') || (matches!(c, '.' | '!' | '?') && next_blank) {
            end = i + 1;
            break;
        }
        if matches!(c, '\n' | '\r') {
            end = i;
            break;
        }
    }
    let sentence: String = chars[..end].iter().collect();
    let sentence = sentence.trim();
    if sentence.chars().count() > STAND_IN_MAX {
        let cut: String = sentence.chars().take(STAND_IN_MAX - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        sentence.to_owned()
    }
}

/// The longest stand-in, in characters.
const STAND_IN_MAX: usize = 120;
