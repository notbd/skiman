//! The rule engine (spec Rules): format rules for skills, the config file
//! formats, unresolved conflicts, secrets, Consistency and the shared body.
//!
//! Entry points:
//! - [`check_all`]: every rule on a whole [`Tree`] (the warnings a write
//!   command prints); [`check_repo`] adds the unused secret exemptions
//!   (`skiman check`).
//! - [`check`]: only what a [`Scope`] names: what a write command writes.
//! - [`allow_hint`]: the ready-to-paste `[[secrets.allow]]` entries for the
//!   secret violations in a list.
//!
//! [`warnings`] and [`warnings_all`] return what is printed as a warning but
//! never fails: the shared body at 80 % of its size limit.
//!
//! Both work on any [`Tree`]: the working tree, an [`crate::tree::Overlay`]
//! of content about to be written, or another snapshot.

mod config;
mod consistency;
pub(crate) mod frontmatter;
mod secrets;
mod shared_body;
mod skill;

use std::collections::BTreeSet;
use std::fmt;
use std::io;

use crate::Exit;
use crate::failure::Failure;
use crate::tree::{Kind, Tree};

pub use config::{LOCKFILE, MANIFEST};
pub use secrets::{AllowEntry, DETECTORS, DETECTORS_VERSION, Detector};

/// Rule ids, printed with every violation.
pub mod rule {
    pub const SKILL_FOLDER: &str = "skill-folder";
    pub const SKILL_MD: &str = "skill-md";
    pub const FRONTMATTER: &str = "frontmatter";
    pub const DUPLICATE_KEY: &str = "duplicate-key";
    pub const REQUIRED_FIELD: &str = "required-field";
    pub const NAME_FORMAT: &str = "name-format";
    pub const NAME_RESERVED: &str = "name-reserved";
    pub const NAME_FOLDER: &str = "name-folder";
    pub const DESCRIPTION_LENGTH: &str = "description-length";
    pub const SYMLINK: &str = "symlink";
    pub const FILE_SIZE: &str = "file-size";
    pub const FILE_COUNT: &str = "file-count";
    pub const SKILL_SIZE: &str = "skill-size";
    pub const UNRESOLVED_CONFLICT: &str = "unresolved-conflict";
    pub const TOML_SYNTAX: &str = "toml-syntax";
    pub const UNKNOWN_KEY: &str = "unknown-key";
    pub const TOML_VALUE: &str = "toml-value";
    pub const LOCK_VERSION: &str = "lock-version";
    pub const CONSISTENCY: &str = "consistency";
    pub const SNAPSHOT_HASH: &str = "snapshot-hash";
    /// The shared body is over the smallest instruction size limit among
    /// enabled tools (a warning from 80 % of it).
    pub const SHARED_BODY_SIZE: &str = "shared-body-size";
    /// A token in the shared body that Claude Code reads as a file import.
    pub const AT_IMPORT: &str = "at-import";
    /// The only rule that exits 3: a source table, lock entry or upstream
    /// snapshot whose skill folder is gone.
    pub const LEFTOVER_RECORD: &str = "leftover-record";
}

/// Size limits per skill (spec Rules defaults).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub file_bytes: u64,
    pub files: usize,
    pub skill_bytes: u64,
}

pub const LIMITS: Limits = Limits {
    file_bytes: 10 * 1024 * 1024,
    files: 1000,
    skill_bytes: 50 * 1024 * 1024,
};

/// One rule violation. Printed as `<path>[:<line>]: <rule>: <message>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    /// Relative to the skill repo root.
    pub path: String,
    /// 1-based line, where a line applies.
    pub line: Option<usize>,
    pub rule: &'static str,
    pub message: String,
    /// For a secret match: the entry that would exempt it.
    pub allow: Option<AllowEntry>,
}

impl Violation {
    pub fn new(
        path: impl Into<String>,
        line: Option<usize>,
        rule: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Violation {
            path: path.into(),
            line,
            rule,
            message: message.into(),
            allow: None,
        }
    }

    /// A secret match that no `[[secrets.allow]]` entry covers.
    fn secret(found: &secrets::Found) -> Self {
        Violation {
            path: found.path.clone(),
            line: Some(found.line),
            rule: found.rule,
            message: format!(
                "looks like a secret; fingerprint = \"{}\"",
                found.fingerprint
            ),
            allow: Some(AllowEntry::for_match(found)),
        }
    }

    /// Leftover records need attention (exit 3); every other violation is exit 1.
    pub fn exit(&self) -> Exit {
        if self.rule == rule::LEFTOVER_RECORD {
            Exit::NeedsAttention
        } else {
            Exit::Refused
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}: {}: {}", self.path, self.rule, self.message),
            None => write!(f, "{}: {}: {}", self.path, self.rule, self.message),
        }
    }
}

/// The highest exit status among `violations` (0 when there are none).
pub fn exit_of(violations: &[Violation]) -> Exit {
    violations
        .iter()
        .map(Violation::exit)
        .fold(Exit::Success, Exit::combine)
}

/// The rules [`Scope::content_only_skills`] applies besides the secret rule.
const CONTENT_RULES: &[&str] = &[
    rule::SYMLINK,
    rule::FILE_SIZE,
    rule::FILE_COUNT,
    rule::SKILL_SIZE,
];

/// What a write command writes, and so what [`check`] validates.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// Files created or changed (repo-relative, e.g. `skills/pdf/SKILL.md`,
    /// `upstream/pdf/run.sh`, `instructions/shared.md`): the per-file rules,
    /// including the frontmatter rules for a skill's `SKILL.md` and the
    /// shared body rules for `instructions/shared.md`, and the secret rule.
    pub files: BTreeSet<String>,
    /// Skill folders (by name) checked as a whole: every file in them plus
    /// the folder rules (name, `SKILL.md` present, file count, total size).
    pub skills: BTreeSet<String>,
    /// Skill folders written as a whole whose files are otherwise unchanged
    /// (`update`: the files the merge changed go in `files`): only the
    /// secret, symlink and size rules on every file and on the folder.
    pub content_only_skills: BTreeSet<String>,
    /// Upstream snapshots `upstream/<name>` written as a whole: the secret
    /// rule on every file in them.
    pub upstreams: BTreeSet<String>,
    /// `[skills.<name>]` tables of `skiman.toml` written.
    pub manifest_tables: BTreeSet<String>,
    /// The rest of `skiman.toml` written: top-level keys, `[tools]`, `[secrets]`.
    pub manifest: bool,
    /// `[skills.<name>]` entries of `skiman.lock` written.
    pub lock_entries: BTreeSet<String>,
    /// The rest of `skiman.lock` written: `version` and top-level keys.
    pub lock: bool,
    /// Skill names whose Consistency is checked.
    pub consistency: BTreeSet<String>,
}

impl Scope {
    /// Everything in `tree`: what `skiman check` validates.
    pub fn all(tree: &dyn Tree) -> io::Result<Scope> {
        let records = config::Records::read(tree)?;
        let skills = folder_names(tree, "skills")?;
        let mut consistency: BTreeSet<String> = skills.iter().cloned().collect();
        let upstreams = folder_names(tree, "upstream")?;
        consistency.extend(upstreams.iter().cloned());
        consistency.extend(records.manifest_skills());
        consistency.extend(records.lock_skills());
        let files = tree
            .walk("instructions")?
            .into_iter()
            .filter(|e| matches!(e.kind, Kind::File { .. }) && !crate::tree::is_ignored(&e.path))
            .map(|e| e.path)
            .collect();
        Ok(Scope {
            files,
            skills,
            content_only_skills: BTreeSet::new(),
            upstreams,
            manifest_tables: records.manifest_skills(),
            manifest: true,
            lock_entries: records.lock_skills(),
            lock: true,
            consistency,
        })
    }
}

/// Names of the entries directly in `dir`, except `.DS_Store` and `Thumbs.db`.
fn folder_names(tree: &dyn Tree, dir: &str) -> io::Result<BTreeSet<String>> {
    Ok(tree
        .children(dir)?
        .into_iter()
        .filter(|e| !crate::tree::is_ignored(&e.path))
        .map(|e| crate::tree::basename(&e.path).to_owned())
        .collect())
}

/// Every rule on everything in `tree`, sorted by path and line.
pub fn check_all(tree: &dyn Tree) -> io::Result<Vec<Violation>> {
    check(tree, &Scope::all(tree)?)
}

/// What `skiman check` reports.
#[derive(Debug, Default)]
pub struct Report {
    /// Every violation in the tree, sorted by path and line.
    pub violations: Vec<Violation>,
    /// `[[secrets.allow]]` entries that cover no match: listed, not violations.
    pub unused_allows: Vec<UnusedAllow>,
}

/// A `[[secrets.allow]]` entry none of whose covered paths has its match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedAllow {
    pub entry: AllowEntry,
    /// Line of the entry in `skiman.toml`.
    pub line: Option<usize>,
}

impl fmt::Display for UnusedAllow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{MANIFEST}")?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        write!(
            f,
            ": unused secret exemption: no file it covers has a `{}` match with fingerprint {} \
             (path = {}); delete the entry",
            self.entry.rule,
            self.entry.fingerprint,
            toml_edit::Value::from(self.entry.path.as_str())
        )
    }
}

/// Every rule on everything in `tree`, plus the unused secret exemptions.
pub fn check_repo(tree: &dyn Tree) -> io::Result<Report> {
    let (violations, found, records) = run(tree, &Scope::all(tree)?)?;
    let unused_allows = records
        .secret_allows()
        .into_iter()
        .filter(|(entry, _)| !found.iter().any(|f| entry.covers(f)))
        .map(|(entry, line)| UnusedAllow { entry, line })
        .collect();
    Ok(Report {
        violations,
        unused_allows,
    })
}

/// The rules on what `scope` names, sorted by path and line.
pub fn check(tree: &dyn Tree, scope: &Scope) -> io::Result<Vec<Violation>> {
    Ok(run(tree, scope)?.0)
}

/// The violations, every secret match (exempted or not) and the config records.
fn run(
    tree: &dyn Tree,
    scope: &Scope,
) -> io::Result<(Vec<Violation>, Vec<secrets::Found>, config::Records)> {
    let mut out = BTreeSet::new();
    let mut found = Vec::new();
    let records = config::Records::read(tree)?;
    for name in &scope.skills {
        skill::check_skill(tree, name, &mut out, &mut found)?;
    }
    for name in &scope.content_only_skills {
        let mut all = BTreeSet::new();
        skill::check_skill(tree, name, &mut all, &mut found)?;
        out.extend(all.into_iter().filter(|v| CONTENT_RULES.contains(&v.rule)));
    }
    for path in &scope.files {
        if path == shared_body::PATH {
            shared_body::check(tree, &records, &mut out)?;
        }
        match tree.kind(path)? {
            None => {}
            Some(_) if path.starts_with("skills/") => {
                skill::check_written_file(tree, path, &mut out, &mut found)?;
            }
            Some(Kind::File { .. })
                if path.starts_with("upstream/") || path.starts_with("instructions/") =>
            {
                secrets::scan(path, &tree.read(path)?, &mut found);
            }
            Some(_) => {}
        }
    }
    for name in &scope.upstreams {
        for entry in tree.walk(&format!("upstream/{name}"))? {
            if matches!(entry.kind, Kind::File { .. }) && !crate::tree::is_ignored(&entry.path) {
                secrets::scan(&entry.path, &tree.read(&entry.path)?, &mut found);
            }
        }
    }
    records.check(scope, &mut out);
    consistency::check(tree, &records, &scope.consistency, &mut out)?;
    let allows = records.secret_allows();
    for f in &found {
        if !allows.iter().any(|(entry, _)| entry.covers(f)) {
            out.insert(Violation::secret(f));
        }
    }
    Ok((out.into_iter().collect(), found, records))
}

/// The unresolved-conflict violations of the skill folder `skills/<name>`
/// (spec Merge semantics: markers, conflict records, `.skiman-conflicts`).
pub fn unresolved_conflicts(tree: &dyn Tree, name: &str) -> io::Result<Vec<Violation>> {
    let scope = Scope {
        skills: BTreeSet::from([name.to_owned()]),
        ..Scope::default()
    };
    let mut violations = check(tree, &scope)?;
    violations.retain(|v| v.rule == rule::UNRESOLVED_CONFLICT);
    Ok(violations)
}

/// The ready-to-paste `[[secrets.allow]]` entries for the secret violations
/// in `violations` (one per distinct entry), with a line saying what they
/// are for; `None` when there are none.
pub fn allow_hint(violations: &[Violation]) -> Option<String> {
    let entries: BTreeSet<&AllowEntry> =
        violations.iter().filter_map(|v| v.allow.as_ref()).collect();
    if entries.is_empty() {
        return None;
    }
    let blocks: Vec<String> = entries.iter().map(|e| e.to_string()).collect();
    Some(format!(
        "If a match is not a real secret, add its entry to {MANIFEST}:\n\n{}",
        blocks.join("\n\n")
    ))
}

/// Warnings on what `scope` names, sorted by path and line.
pub fn warnings(tree: &dyn Tree, scope: &Scope) -> io::Result<Vec<Violation>> {
    let mut out = BTreeSet::new();
    if scope.files.contains(shared_body::PATH) {
        shared_body::warnings(tree, &config::Records::read(tree)?, &mut out)?;
    }
    Ok(out.into_iter().collect())
}

/// Warnings on everything in `tree`.
pub fn warnings_all(tree: &dyn Tree) -> io::Result<Vec<Violation>> {
    warnings(tree, &Scope::all(tree)?)
}

/// `true` when `kind` is a regular file within the per-file size limit, so
/// its content is read.
fn readable(kind: Kind) -> bool {
    matches!(kind, Kind::File { size, .. } if size <= LIMITS.file_bytes)
}

/// The rules as a write command applies them before writing (spec Rules):
/// what `scope` names must pass, otherwise the command is refused (exit 1)
/// and nothing is written; every other violation in `tree` is printed to
/// stderr as a warning and never blocks. `tree` shows the content as it
/// will be after the write (for example an [`crate::tree::Overlay`]).
pub fn validate_write(tree: &dyn Tree, scope: &Scope) -> Result<(), Failure> {
    let io = |e: io::Error| Failure::operational(format!("cannot read the skill repo: {e}"));
    let blocking = check(tree, scope).map_err(io)?;
    if !blocking.is_empty() {
        let lines: Vec<String> = blocking.iter().map(|v| format!("  {v}")).collect();
        let mut message = format!(
            "refusing to write content that breaks the rules:\n{}",
            lines.join("\n")
        );
        if let Some(hint) = allow_hint(&blocking) {
            message.push_str("\n\n");
            message.push_str(&hint);
        }
        return Err(Failure::refused(message));
    }
    for violation in check_all(tree).map_err(io)? {
        eprintln!("warning: {violation}");
    }
    for warning in warnings_all(tree).map_err(io)? {
        eprintln!("warning: {warning}");
    }
    Ok(())
}
