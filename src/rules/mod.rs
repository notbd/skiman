//! The rule engine (spec Rules): format rules for skills, the config file
//! formats, unresolved conflicts and Consistency. Secrets (#7) and the shared
//! body (#10) plug in here.
//!
//! Two entry points:
//! - [`check_all`]: every rule on a whole [`Tree`] (`skiman check`, and the
//!   warnings a write command prints).
//! - [`check`]: only what a [`Scope`] names: what a write command writes.
//!
//! Both work on any [`Tree`]: the working tree, an [`crate::tree::Overlay`]
//! of content about to be written, or another snapshot.

mod config;
mod consistency;
pub(crate) mod frontmatter;
mod skill;

use std::collections::BTreeSet;
use std::fmt;
use std::io;

use crate::Exit;
use crate::failure::Failure;
use crate::tree::{Kind, Tree};

pub use config::{LOCKFILE, MANIFEST};

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

/// What a write command writes, and so what [`check`] validates.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// Files created or changed (repo-relative, e.g. `skills/pdf/SKILL.md`):
    /// the per-file rules, including the frontmatter rules for a skill's `SKILL.md`.
    pub files: BTreeSet<String>,
    /// Skill folders (by name) checked as a whole: every file in them plus
    /// the folder rules (name, `SKILL.md` present, file count, total size).
    pub skills: BTreeSet<String>,
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
        consistency.extend(folder_names(tree, "upstream")?);
        consistency.extend(records.manifest_skills());
        consistency.extend(records.lock_skills());
        Ok(Scope {
            files: BTreeSet::new(),
            skills,
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

/// The rules on what `scope` names, sorted by path and line.
pub fn check(tree: &dyn Tree, scope: &Scope) -> io::Result<Vec<Violation>> {
    let mut out = BTreeSet::new();
    for name in &scope.skills {
        skill::check_skill(tree, name, &mut out)?;
    }
    for path in &scope.files {
        if tree.kind(path)?.is_some() {
            skill::check_written_file(tree, path, &mut out)?;
        }
    }
    let records = config::Records::read(tree)?;
    records.check(scope, &mut out);
    consistency::check(tree, &records, &scope.consistency, &mut out)?;
    Ok(out.into_iter().collect())
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
        return Err(Failure::refused(format!(
            "refusing to write content that breaks the rules:\n{}",
            lines.join("\n")
        )));
    }
    for violation in check_all(tree).map_err(io)? {
        eprintln!("warning: {violation}");
    }
    Ok(())
}
