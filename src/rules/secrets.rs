//! The secret rule (spec Rules, "No secrets"): a detector list shipped as
//! data, matches reported by fingerprint (never by the matched text), and the
//! `[[secrets.allow]]` exemptions that cover them.

use std::fmt;
use std::sync::LazyLock;

use regex::bytes::{Captures, Regex};
use sha2::{Digest, Sha256};

use super::skill::{CONFLICT_RECORD_SUFFIXES, CONFLICTS_DIR};
use crate::tree::hex;

/// One secret detector: the rule id printed with its matches, the pattern,
/// and the extra conditions a match must meet.
pub struct Detector {
    pub id: &'static str,
    pub pattern: &'static str,
    /// Extra conditions on a match (`content` is the whole file, so the
    /// bytes around the match can be looked at): `None` rejects it;
    /// otherwise the end of the secret, the text its fingerprint covers,
    /// which starts with the match.
    pub accept: fn(content: &[u8], m: &Captures) -> Option<usize>,
    /// Where the search goes on after a match `accept` rejects.
    pub on_reject: Resume,
}

/// Where the search for a detector's next match starts after a rejected one.
pub enum Resume {
    /// One byte after its start: a later match may start inside it.
    AfterStart,
    /// At its end: every match that starts inside it is rejected too.
    AtEnd,
}

/// The v1 detectors, with the patterns and conditions of the spec.
pub const DETECTORS: &[Detector] = &[
    Detector {
        id: "private-key",
        pattern: r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        accept: private_key,
        on_reject: Resume::AtEnd,
    },
    Detector {
        id: "aws-access-key",
        pattern: r"AKIA[0-9A-Z]{16}",
        accept: always,
        on_reject: Resume::AtEnd,
    },
    Detector {
        id: "github-token",
        pattern: r"ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{22,}",
        accept: always,
        on_reject: Resume::AtEnd,
    },
    Detector {
        id: "openai-key",
        pattern: r"sk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}",
        accept: openai_key,
        on_reject: Resume::AtEnd,
    },
    Detector {
        id: "slack-token",
        pattern: r"xox[abprs]-[A-Za-z0-9-]{10,}",
        accept: always,
        on_reject: Resume::AtEnd,
    },
    Detector {
        id: "secret-assignment",
        pattern: r#"(?i)(password|secret|token)\s*[:=]\s*["']([^"'\s]{8,})["']"#,
        accept: secret_assignment,
        on_reject: Resume::AfterStart,
    },
];

fn always(_: &[u8], m: &Captures) -> Option<usize> {
    Some(m.get(0).expect("group 0 is the match").end())
}

/// The whole key block, from its BEGIN line to the last `-` of its END
/// line, when the first `-----` after the header starts the END line of
/// the same label: so an entry for an example header does not cover a key
/// pasted below it. Otherwise (another BEGIN, an END of another label, no
/// END) the header alone, as the block has no reliable end. PEM bodies and
/// headers (`Proc-Type`, `DEK-Info`) hold no `-----`, and the next header
/// starts with one, so the searches of all headers in a file never overlap.
fn private_key(content: &[u8], m: &Captures) -> Option<usize> {
    let header = m.get(0).expect("group 0 is the match");
    let label = header
        .as_bytes()
        .strip_prefix(b"-----BEGIN ")
        .and_then(|rest| rest.strip_suffix(b"PRIVATE KEY-----"))
        .expect("the pattern has this shape");
    let end_line = [b"-----END ".as_slice(), label, b"PRIVATE KEY-----"].concat();
    let after = &content[header.end()..];
    let block = after
        .windows(5)
        .position(|w| w == b"-----")
        .filter(|&dashes| after[dashes..].starts_with(&end_line))
        .map_or(0, |dashes| dashes + end_line.len());
    Some(header.end() + block)
}

/// Only when not preceded by `[A-Za-z0-9_-]`, and when the part after `sk-`
/// holds at least one uppercase letter and one digit. Every byte of a match
/// is such a character, so a match that starts inside a rejected one is
/// preceded by one and rejected too.
fn openai_key(content: &[u8], m: &Captures) -> Option<usize> {
    let whole = m.get(0).expect("group 0 is the match");
    let preceded = whole.start() > 0 && is_key_byte(content[whole.start() - 1]);
    let rest = &whole.as_bytes()[3..];
    (!preceded && rest.iter().any(u8::is_ascii_uppercase) && rest.iter().any(u8::is_ascii_digit))
        .then_some(whole.end())
}

fn is_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Skips values that start with `$`, `<`, `{` or `%`, or contain `example`,
/// `your` or `xxx` (in any case, like the rest of the detector).
fn secret_assignment(_: &[u8], m: &Captures) -> Option<usize> {
    let value = m.get(2).expect("the pattern has a value group").as_bytes();
    let lower = value.to_ascii_lowercase();
    let contains = |word: &[u8]| lower.windows(word.len()).any(|w| w == word);
    let real = !matches!(value[0], b'$' | b'<' | b'{' | b'%')
        && !contains(b"example")
        && !contains(b"your")
        && !contains(b"xxx");
    real.then(|| m.get(0).expect("group 0 is the match").end())
}

static COMPILED: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    DETECTORS
        .iter()
        .map(|d| Regex::new(d.pattern).expect("detector patterns are valid"))
        .collect()
});

/// One detector match in one file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Found {
    /// Relative to the canon root.
    pub path: String,
    /// 1-based line of the start of the match.
    pub line: usize,
    pub rule: &'static str,
    /// `sha256:<hex of the secret's text>`: the matched text, or a whole
    /// private key block.
    pub fingerprint: String,
    /// The path an exemption names for this match ([`allow_path`], without
    /// the skill's group; the rule engine sets it once it knows the groups).
    pub allow_path: String,
}

/// Every detector match in `content`, the file at `path`.
pub fn scan(path: &str, content: &[u8], found: &mut Vec<Found>) {
    for (detector, regex) in DETECTORS.iter().zip(COMPILED.iter()) {
        let mut at = 0;
        // Line numbers are counted on from the previous match, never from
        // the start of the file again.
        let (mut line, mut counted) = (1, 0);
        while let Some(m) = regex.captures_at(content, at) {
            let whole = m.get(0).expect("group 0 is the match");
            if let Some(end) = (detector.accept)(content, &m) {
                line += content[counted..whole.start()]
                    .iter()
                    .filter(|b| **b == b'\n')
                    .count();
                counted = whole.start();
                let secret = &content[whole.start()..end];
                found.push(Found {
                    path: path.to_owned(),
                    allow_path: allow_path(path),
                    line,
                    rule: detector.id,
                    fingerprint: format!("sha256:{}", hex(&Sha256::digest(secret))),
                });
                at = end;
            } else {
                at = match detector.on_reject {
                    Resume::AfterStart => whole.start() + 1,
                    Resume::AtEnd => whole.end(),
                };
            }
        }
    }
}

/// The path a ready-to-paste entry uses for a match at `path` (given
/// without a group, see [`crate::layout::Layout::without_group`]): the
/// `skills/<n>/<p>` form for matches in a skill, its upstream snapshot or
/// its conflict records, so an exemption stays put when the skill changes
/// group; the path itself elsewhere (`instructions/`).
pub fn allow_path(path: &str) -> String {
    let Some((name, rest)) = path
        .strip_prefix("upstream/")
        .or_else(|| path.strip_prefix("skills/"))
        .and_then(|rest| rest.split_once('/'))
    else {
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
            path: found.allow_path.clone(),
            rule: found.rule.to_owned(),
            fingerprint: found.fingerprint.clone(),
        }
    }

    /// Whether this entry exempts `found`: same rule and fingerprint, and
    /// `found` is at the entry's path or at a path the entry covers.
    pub fn covers(&self, found: &Found) -> bool {
        self.rule == found.rule
            && self.fingerprint == found.fingerprint
            && (self.path == found.path || self.path == found.allow_path)
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
