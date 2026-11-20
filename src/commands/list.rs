//! `skiman list`: one row per skill folder under `skills/` with its kind,
//! source, followed ref or pinned version, whether it has local
//! modifications or unresolved conflicts, and whether it is enabled; then
//! one row per linked skill (kind `linked`, its folder's id and path as the
//! source, and whether it is distributed), and one per linked folder that
//! is not on this machine. Read-only; takes no lock and spawns no `git`.
//! `overview` reads the canon's skills through the same [`canon_skills`].

use crate::Exit;
use crate::canon::Canon;
use crate::failure::{Failure, finish};
use crate::layout::Placed;
use crate::linked::{Holder, Linked, Shown};
use crate::lockfile::{LockEntry, Lockfile};
use crate::manifest::{Manifest, SourceRecord};
use crate::paths::Paths;
use crate::rules;
use crate::sync;
use crate::tree::{Kind, Tree, WorkingTree, tree_hash};

const HEADER: [&str; 7] = [
    "NAME",
    "KIND",
    "SOURCE",
    "VERSION",
    "MODIFIED",
    "CONFLICTS",
    "ENABLED",
];

pub fn run() -> Exit {
    finish(list())
}

fn list() -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = Canon::from_config(&paths)?;
    let manifest = Manifest::load(&repo)?;
    let lockfile = Lockfile::load(&repo)?;
    let mut rows = vec![HEADER.map(str::to_owned)];
    for skill in canon_skills(&repo, &manifest)? {
        let (source, version) = match &skill.source {
            None => ("-".to_owned(), "-".to_owned()),
            Some(source) => {
                let entry = lockfile.entry(&skill.placed.name);
                (source_text(source), version(source, entry.as_ref()))
            }
        };
        rows.push([
            skill.placed.name.clone(),
            skill.kind().to_owned(),
            source,
            version,
            skill.modified,
            yes_no(skill.conflicts),
            yes_no(skill.enabled),
        ]);
    }
    let linked = Linked::read(paths.home(), &repo, &manifest);
    let kept = sync::kept_names(&paths, &repo, &manifest, &linked);
    for (folder, shown) in linked.shown(&repo.layout()?, &kept) {
        let source = format!(
            "{} ({})",
            folder.id,
            folder
                .path
                .as_ref()
                .expect("a shown folder has a path")
                .display()
        );
        let row = |name: String, state: String| {
            let dash = || "-".to_owned();
            [
                name,
                "linked".to_owned(),
                source.clone(),
                dash(),
                dash(),
                dash(),
                state,
            ]
        };
        match shown {
            Shown::Missing => rows.push(row("-".to_owned(), "not on this machine".to_owned())),
            Shown::Skills(skills) => rows.extend(
                skills
                    .into_iter()
                    .map(|(name, lost)| row(name, distributed(lost))),
            ),
        }
    }
    print_columns(&rows);
    Ok(Exit::Success)
}

/// A skill folder under `skills/` as `list` and `overview` show it.
pub(crate) struct CanonSkill {
    pub placed: Placed,
    /// `None` for an own skill.
    pub source: Option<SourceRecord>,
    /// For a third-party skill, whether it differs from its upstream
    /// snapshot (tree hash; `unknown` without one); `-` for an own skill.
    pub modified: String,
    pub conflicts: bool,
    pub enabled: bool,
}

impl CanonSkill {
    /// `own`, or `third-party` for a skill with a source.
    pub fn kind(&self) -> &'static str {
        self.source.as_ref().map_or("own", |_| "third-party")
    }
}

/// Every skill folder under `skills/`, in layout order.
pub(crate) fn canon_skills(repo: &Canon, manifest: &Manifest) -> Result<Vec<CanonSkill>, Failure> {
    let tree = WorkingTree::new(repo.root());
    let io = |e| Failure::operational(format!("cannot read {}: {e}", repo.root().display()));
    let enabled = manifest.skills();
    let mut skills = Vec::new();
    for placed in repo.layout()?.skills() {
        let path = placed.path();
        if tree.kind(&path).map_err(io)? != Some(Kind::Dir) {
            continue;
        }
        let source = manifest.source(&placed.name);
        let upstream = format!("upstream/{}", placed.name);
        let modified = match source {
            None => "-".to_owned(),
            Some(_) if tree.kind(&upstream).map_err(io)? != Some(Kind::Dir) => "unknown".to_owned(),
            Some(_) => yes_no(
                tree_hash(&tree, &path).map_err(io)? != tree_hash(&tree, &upstream).map_err(io)?,
            ),
        };
        skills.push(CanonSkill {
            conflicts: !rules::unresolved_conflicts(&tree, &path)
                .map_err(io)?
                .is_empty(),
            enabled: enabled.get(&placed.name).is_none_or(|e| e.enabled),
            placed: placed.clone(),
            source,
            modified,
        });
    }
    Ok(skills)
}

/// Whether a linked skill is distributed: `no: <what holds its name>` when
/// it lost its name.
pub(crate) fn distributed(lost: Option<Holder>) -> String {
    lost.map_or_else(|| yes_no(true), |holder| format!("no: {holder}"))
}

pub(crate) fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_owned()
}

/// The URL, and the folder inside the upstream when it is not the root.
pub(crate) fn source_text(source: &SourceRecord) -> String {
    if source.path.is_empty() {
        source.url.clone()
    } else {
        format!("{} ({})", source.url, source.path)
    }
}

/// The followed ref or pinned version, with the locked commit. `ref_kind`
/// describes the ref of the last update, so it is shown only while
/// `source.ref` is still the lock's `ref`.
fn version(source: &SourceRecord, entry: Option<&LockEntry>) -> String {
    let commit = entry.and_then(|e| e.commit.as_deref());
    let at = commit
        .map(|c| format!(" (at {})", short(c)))
        .unwrap_or_default();
    let locked = entry.is_some_and(|e| e.git_ref == source.git_ref);
    let kind = source.ref_kind.filter(|_| locked);
    match (source.git_ref.as_deref(), kind) {
        (None, _) if commit.is_none() => "local folder".to_owned(),
        (None, _) => format!("follows the default branch{at}"),
        (Some(r), Some("branch")) => format!("follows branch {r}{at}"),
        (Some(r), Some("tag")) => match commit {
            Some(c) => format!("pinned to tag {r} ({})", short(c)),
            None => format!("pinned to tag {r}"),
        },
        (Some(r), Some(_)) => format!("pinned to commit {}", short(r)),
        // A hand-written or changed ref: its kind is derived at the next update.
        (Some(r), None) => format!("ref {r}{at}"),
    }
}

fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// Left-aligned columns separated by two spaces; the last column is not padded.
fn print_columns(rows: &[[String; 7]]) {
    let mut widths = [0; 7];
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            if i + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{cell:<width$}  ", width = widths[i]));
            }
        }
        out!("{line}");
    }
}
