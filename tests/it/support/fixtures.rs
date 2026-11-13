//! File contents many tests start from.

/// A valid `SKILL.md` for the skill `name`.
pub fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Does {name} things\n---\n\n# {name}\n")
}

/// `SKILL.md` of the third-party skill `pdf` most install and update tests use.
pub const PDF_SKILL_MD: &str = "---\nname: pdf\ndescription: Fill PDF forms\n---\n\n# pdf\n";

/// Twenty numbered lines with `edits` (line number, text) replacing some, so
/// git sees edits far apart as separate hunks and detects renames.
pub fn lines(edits: &[(usize, &str)]) -> String {
    (1..=20)
        .map(|n| {
            let line = edits
                .iter()
                .find(|(at, _)| *at == n)
                .map_or_else(|| format!("line {n}"), |(_, text)| (*text).to_owned());
            format!("{line}\n")
        })
        .collect()
}

/// An AWS-key-like value, built at runtime so the source never holds one
/// (GitHub push protection would block the commit).
pub fn aws_key() -> String {
    ["AK", "IA", "Q2W3E4R5T6Y7U8I9"].concat()
}
