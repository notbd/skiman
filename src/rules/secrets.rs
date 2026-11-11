//! The secret rule (spec Rules, "No secrets"): a detector list shipped as
//! data, matches reported by fingerprint (never by the matched text), and the
//! `[[secrets.allow]]` exemptions that cover them.

use std::fmt;
use std::sync::LazyLock;

use regex::bytes::{Captures, Regex, RegexBuilder};
use sha2::{Digest, Sha256};

use super::skill::{CONFLICT_RECORD_SUFFIXES, CONFLICTS_DIR};
use crate::tree::hex;

/// One secret detector: the rule id printed with its matches, the pattern,
/// and the extra conditions a match must meet.
pub struct Detector {
    pub id: &'static str,
    pub pattern: &'static str,
    pub case_insensitive: bool,
    /// Extra conditions on a match (`content` is the whole file, so the
    /// byte before the match can be looked at).
    pub accept: fn(content: &[u8], m: &Captures) -> bool,
}

/// The v1 detectors, with the patterns and conditions of the spec.
pub const DETECTORS: &[Detector] = &[
    Detector {
        id: "private-key",
        pattern: r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        case_insensitive: false,
        accept: always,
    },
    Detector {
        id: "aws-access-key",
        pattern: r"AKIA[0-9A-Z]{16}",
        case_insensitive: false,
        accept: always,
    },
    Detector {
        id: "github-token",
        pattern: r"ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{22,}",
        case_insensitive: false,
        accept: always,
    },
    Detector {
        id: "openai-key",
        pattern: r"sk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}",
        case_insensitive: false,
        accept: openai_key,
    },
    Detector {
        id: "slack-token",
        pattern: r"xox[abprs]-[A-Za-z0-9-]{10,}",
        case_insensitive: false,
        accept: always,
    },
    Detector {
        id: "secret-assignment",
        pattern: r#"(password|secret|token)\s*[:=]\s*["']([^"'\s]{8,})["']"#,
        case_insensitive: true,
        accept: secret_assignment,
    },
];

fn always(_: &[u8], _: &Captures) -> bool {
    true
}

/// Only when not preceded by `[A-Za-z0-9_-]`, and when the part after `sk-`
/// holds at least one uppercase letter and one digit.
fn openai_key(content: &[u8], m: &Captures) -> bool {
    let whole = m.get(0).expect("group 0 is the match");
    let preceded = whole.start() > 0 && is_key_byte(content[whole.start() - 1]);
    let rest = &whole.as_bytes()[3..];
    !preceded && rest.iter().any(u8::is_ascii_uppercase) && rest.iter().any(u8::is_ascii_digit)
}

fn is_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Skips values that start with `$`, `<`, `{` or `%`, or contain `example`,
/// `your` or `xxx` (in any case, like the rest of the detector).
fn secret_assignment(_: &[u8], m: &Captures) -> bool {
    let value = m.get(2).expect("the pattern has a value group").as_bytes();
    let lower = value.to_ascii_lowercase();
    let contains = |word: &[u8]| lower.windows(word.len()).any(|w| w == word);
    !matches!(value[0], b'$' | b'<' | b'{' | b'%')
        && !contains(b"example")
        && !contains(b"your")
        && !contains(b"xxx")
}

static COMPILED: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    DETECTORS
        .iter()
        .map(|d| {
            RegexBuilder::new(d.pattern)
                .case_insensitive(d.case_insensitive)
                .build()
                .expect("detector patterns are valid")
        })
        .collect()
});

/// One detector match in one file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Found {
    /// Relative to the skill repo root.
    pub path: String,
    /// 1-based line of the start of the match.
    pub line: usize,
    pub rule: &'static str,
    /// `sha256:<hex of the matched text>`.
    pub fingerprint: String,
}

/// Every detector match in `content`, the file at `path`.
pub fn scan(path: &str, content: &[u8], found: &mut Vec<Found>) {
    for (detector, regex) in DETECTORS.iter().zip(COMPILED.iter()) {
        let mut at = 0;
        while let Some(m) = regex.captures_at(content, at) {
            let whole = m.get(0).expect("group 0 is the match");
            if (detector.accept)(content, &m) {
                found.push(Found {
                    path: path.to_owned(),
                    line: content[..whole.start()]
                        .iter()
                        .filter(|b| **b == b'\n')
                        .count()
                        + 1,
                    rule: detector.id,
                    fingerprint: format!("sha256:{}", hex(&Sha256::digest(whole.as_bytes()))),
                });
                at = whole.end();
            } else {
                // A later match may start inside a rejected one.
                at = whole.start() + 1;
            }
        }
    }
}

/// The path a ready-to-paste entry uses for a match at `path`: the
/// `skills/<n>/<p>` form for matches in a skill, its upstream snapshot or
/// its conflict records; the path itself elsewhere (`instructions/`).
pub fn allow_path(path: &str) -> String {
    let (name, rest) = if let Some(rest) = path.strip_prefix("upstream/") {
        match rest.split_once('/') {
            Some(parts) => parts,
            None => return path.to_owned(),
        }
    } else if let Some(rest) = path.strip_prefix("skills/") {
        match rest.split_once('/') {
            Some(parts) => parts,
            None => return path.to_owned(),
        }
    } else {
        return path.to_owned();
    };
    let rest = rest
        .strip_prefix(CONFLICTS_DIR)
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(rest);
    let record = CONFLICT_RECORD_SUFFIXES[0];
    let parts: Vec<&str> = rest
        .split('/')
        .map(|part| part.strip_suffix(record).unwrap_or(part))
        .collect();
    format!("skills/{name}/{}", parts.join("/"))
}

/// One `[[secrets.allow]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AllowEntry {
    pub path: String,
    pub rule: String,
    pub fingerprint: String,
}

impl AllowEntry {
    /// The entry a refusal prints for `found`.
    pub fn for_match(found: &Found) -> AllowEntry {
        AllowEntry {
            path: allow_path(&found.path),
            rule: found.rule.to_owned(),
            fingerprint: found.fingerprint.clone(),
        }
    }

    /// Whether this entry exempts `found`: same rule and fingerprint, and
    /// `found` is at the entry's path or at a path the entry covers.
    pub fn covers(&self, found: &Found) -> bool {
        self.rule == found.rule
            && self.fingerprint == found.fingerprint
            && (self.path == found.path || self.path == allow_path(&found.path))
    }
}

/// The entry as TOML, ready to paste into `skiman.toml`.
impl fmt::Display for AllowEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let quoted = |s: &str| toml_edit::Value::from(s).to_string();
        write!(
            f,
            "[[secrets.allow]]\npath = {}\nrule = {}\nfingerprint = {}",
            quoted(&self.path),
            quoted(&self.rule),
            quoted(&self.fingerprint)
        )
    }
}
