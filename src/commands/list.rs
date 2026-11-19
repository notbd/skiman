//! `skiman list`: one row per skill folder under `skills/` with its kind,
//! source, followed ref or pinned version, whether it has local
//! modifications or unresolved conflicts, and whether it is enabled; then
//! one row per linked skill (kind `linked`, its folder's id and path as the
//! source, and whether it is distributed), and one per linked folder that
//! is not on this machine. Read-only; takes no lock and spawns no `git`.

use crate::Exit;
use crate::canon::Canon;
use crate::failure::{Failure, finish};
use crate::linked::{Linked, Shown};
use crate::lockfile::Lockfile;
use crate::manifest::{Manifest, SourceRecord};
use crate::paths::Paths;
use crate::rules;
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
    let tree = WorkingTree::new(repo.root());
    let io = |e| Failure::operational(format!("cannot read {}: {e}", repo.root().display()));
    let enabled = manifest.skills();
    let mut rows = vec![HEADER.map(str::to_owned)];
    for placed in repo.layout()?.skills() {
        let name = placed.name.clone();
        if tree.kind(&placed.path()).map_err(io)? != Some(Kind::Dir) {
            continue;
        }
        let conflicts = !rules::unresolved_conflicts(&tree, &name)
            .map_err(io)?
            .is_empty();
        let enabled = enabled.get(&name).is_none_or(|e| e.enabled);
        let [kind, source, version, modified] = match manifest.source(&name) {
            None => ["own", "-", "-", "-"].map(str::to_owned),
            Some(source) => {
                let commit = lockfile.entry(&name).and_then(|entry| entry.commit);
                [
                    "third-party".to_owned(),
                    source_text(&source),
                    version(&source, commit.as_deref()),
                    modified(&tree, &placed.path(), &name).map_err(io)?,
                ]
            }
        };
        let (conflicts, enabled) = (yes_no(conflicts), yes_no(enabled));
        rows.push([name, kind, source, version, modified, conflicts, enabled]);
    }
    let linked = Linked::read(paths.home(), &repo, &manifest);
    for (folder, shown) in linked.shown(&repo.layout()?) {
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
            Shown::Skills(skills) => rows.extend(skills.into_iter().map(|(name, lost)| {
                let state = match lost {
                    Some(holder) => format!("no: {holder}"),
                    None => yes_no(true),
                };
                row(name, state)
            })),
        }
    }
    print_columns(&rows);
    Ok(Exit::Success)
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

/// The followed ref or pinned version, with the locked commit.
fn version(source: &SourceRecord, commit: Option<&str>) -> String {
    let at = commit
        .map(|c| format!(" (at {})", short(c)))
        .unwrap_or_default();
    match (source.git_ref.as_deref(), source.ref_kind) {
        (None, _) if commit.is_none() => "local folder".to_owned(),
        (None, _) => format!("follows the default branch{at}"),
        (Some(r), Some("branch")) => format!("follows branch {r}{at}"),
        (Some(r), Some("tag")) => match commit {
            Some(c) => format!("pinned to tag {r} ({})", short(c)),
            None => format!("pinned to tag {r}"),
        },
        (Some(r), Some(_)) => format!("pinned to commit {}", short(r)),
        // A hand-written ref: its kind is derived at the next update.
        (Some(r), None) => format!("ref {r}{at}"),
    }
}

fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// Whether the skill folder `skill` differs from the upstream snapshot of
/// `name` (tree hash).
pub(crate) fn modified(tree: &dyn Tree, skill: &str, name: &str) -> std::io::Result<String> {
    let upstream = format!("upstream/{name}");
    if tree.kind(&upstream)? != Some(Kind::Dir) {
        return Ok("unknown".to_owned());
    }
    let differs = tree_hash(tree, skill)? != tree_hash(tree, &upstream)?;
    Ok(yes_no(differs))
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
