//! Distribution: the reconcile loop behind `skiman sync`, `sync --hook` and
//! the sync every write command ends with (spec Distribution and refresh).
//!
//! [`run`] is the entry point; it needs the repo lock. [`reconcile`] computes
//! and applies the changes and returns a [`Report`] of what it did and found.
//! Only owned entries and instruction locations are created, retargeted or
//! removed (spec Ownership);
//! everything else is reported as an [`Item`]. Never spawns `git`:
//! `sync --hook` must stay fast.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::failure::Failure;
use crate::fsutil;
use crate::managing_skills;
use crate::manifest::Manifest;
use crate::names;
use crate::ownership::{Entry, Ownership};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::reported::Reported;
use crate::tools::{AgentTool, TOOLS};

/// Who reads sync's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// A command run by the user: changes on stdout, problems on stderr.
    Normal,
    /// `sync --hook`: nothing on stdout unless there is something new to report; exit 0.
    Hook,
}

/// Something sync found and leaves alone. `sync` reports every item;
/// `sync --hook` only those not reported at the last sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Realpath of the tool directory, of `skills/` for invalid skill folders,
    /// or the instruction location (realpath of its parent joined with the file name).
    pub scope: String,
    pub path: PathBuf,
    pub kind: ItemKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemKind {
    /// A foreign skill; `tool` is the id of the tool whose directory holds it.
    Foreign { name: String, tool: &'static str },
    /// Sync wants to link skill `name` here, but something it does not own is there (exit 3).
    Occupied { name: String },
    /// A folder under `skills/` that is never distributed, and why.
    InvalidSkillFolder { reason: &'static str },
    /// An instruction location skiman does not own, so the shared body is not linked there (exit 3).
    RefusedLocation,
    /// A report on a project item or root (`crate::project::sync`): `what`
    /// names the report in the recorded set; `attention` makes it exit 3.
    Project {
        what: &'static str,
        message: String,
        attention: bool,
    },
}

impl Item {
    /// What identifies the item in the recorded set.
    pub fn id(&self) -> String {
        let kind = match self.kind {
            ItemKind::Foreign { .. } => "foreign",
            ItemKind::Occupied { .. } => "occupied",
            ItemKind::InvalidSkillFolder { .. } => "invalid-skill-folder",
            ItemKind::RefusedLocation => "refused-location",
            ItemKind::Project { what, .. } => {
                return format!("project-{what} {}", self.path.display());
            }
        };
        format!("{kind} {}", self.path.display())
    }

    pub fn message(&self) -> String {
        let path = self.path.display();
        match &self.kind {
            ItemKind::Foreign { name, tool } => format!(
                "foreign skill {path} is not managed by skiman; bring it in with `skiman adopt {name} --from {tool}`"
            ),
            ItemKind::Occupied { name } => format!(
                "{path} is occupied by an entry skiman does not own, so skill {name} is not linked there; move it away, then run `skiman sync`"
            ),
            ItemKind::InvalidSkillFolder { reason } => {
                format!("skill folder {path} is not distributed: {reason}")
            }
            ItemKind::RefusedLocation => format!(
                "instruction file {path} is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`"
            ),
            ItemKind::Project { message, .. } => message.clone(),
        }
    }
}

/// A change sync made to an owned tool-directory entry or instruction location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Linked,
    Retargeted,
    Removed,
    /// A project item's change was copied into its mirror.
    Mirrored,
}

/// What one reconcile did and found.
#[derive(Debug, Default)]
pub struct Report {
    /// Changes in the order they were made, with the tool-directory entry or
    /// instruction location path.
    pub changes: Vec<(Change, PathBuf)>,
    pub items: Vec<Item>,
    /// I/O failures (exit 4); sync goes on with the other entries.
    pub errors: Vec<String>,
}

impl Report {
    pub fn exit(&self) -> Exit {
        if !self.errors.is_empty() {
            Exit::OperationalError
        } else if self.items.iter().any(|item| {
            matches!(
                item.kind,
                ItemKind::Occupied { .. }
                    | ItemKind::RefusedLocation
                    | ItemKind::Project {
                        attention: true,
                        ..
                    }
            )
        }) {
            Exit::NeedsAttention
        } else {
            Exit::Success
        }
    }
}

/// Syncs `repo` while the caller holds its lock and prints the result for `output`.
pub fn run(paths: &Paths, repo: &SkillRepo, lock: &RepoLock, output: Output) -> Exit {
    run_with(paths, repo, lock, output, None).0
}

/// [`run`], also returning the report. `previous_repo` is the realpath of the
/// skill repo `init` just switched away from: its links count as owned.
pub fn run_with(
    paths: &Paths,
    repo: &SkillRepo,
    _lock: &RepoLock,
    output: Output,
    previous_repo: Option<PathBuf>,
) -> (Exit, Report) {
    let mut report = match Manifest::load(repo) {
        Ok(manifest) => {
            let mut report = reconcile(paths.home(), repo, &manifest, previous_repo);
            crate::project::sync::reconcile(paths, repo, &manifest, &mut report);
            report
        }
        Err(failure) => return (report_failure(failure, output), Report::default()),
    };
    if output == Output::Normal {
        for (change, entry) in &report.changes {
            let verb = match change {
                Change::Linked => "linked",
                Change::Retargeted => "retargeted",
                Change::Removed => "removed",
                Change::Mirrored => "mirrored",
            };
            println!("{verb} {}", entry.display());
        }
    }
    let last = Reported::load(paths);
    let mut now = Reported::default();
    for item in &report.items {
        let id = item.id();
        match output {
            Output::Normal => eprintln!("warning: {}", item.message()),
            Output::Hook if !last.contains(&item.scope, &id) => {
                println!("skiman: {}", item.message())
            }
            Output::Hook => {}
        }
        now.insert(&item.scope, id);
    }
    if now != last
        && let Err(e) = now.save(paths)
    {
        report
            .errors
            .push(format!("cannot write {}: {e}", paths.state_dir().display()));
    }
    for error in &report.errors {
        eprintln!("error: {error}");
    }
    let exit = match output {
        Output::Normal => report.exit(),
        Output::Hook => Exit::Success,
    };
    (exit, report)
}

/// Prints a failure that stops sync before it reconciles anything.
pub fn report_failure(failure: Failure, output: Output) -> Exit {
    let exit = failure.report();
    match output {
        Output::Normal => exit,
        Output::Hook => Exit::Success,
    }
}

/// Materialises `managing-skills`, then makes every installed tool's tool
/// directory hold an owned link for every wanted entry, removes owned entries
/// nothing wants, and removes owned entries from tool directories of known
/// tools that are not enabled. Then does the same for instruction locations
/// and the shared body.
pub fn reconcile(
    home: &Path,
    repo: &SkillRepo,
    manifest: &Manifest,
    previous_repo: Option<PathBuf>,
) -> Report {
    let mut report = Report::default();
    if let Err(e) = managing_skills::materialise(repo) {
        report.errors.push(format!(
            "cannot write {}: {e}",
            repo.managing_skills_dir().display()
        ));
    }
    let wanted = match wanted_entries(repo, manifest, &mut report) {
        Ok(wanted) => wanted,
        Err(e) => {
            report
                .errors
                .push(format!("cannot read {}: {e}", repo.skills_dir().display()));
            return report;
        }
    };
    let ownership = Ownership::new(home, repo, manifest, previous_repo);
    // Tool directories that resolve to the same real directory are reconciled once.
    let mut reconciled = Vec::new();
    for tool in installed_tools(home, manifest) {
        let dir = tool.tool_dir(home);
        // The tool directory itself may be missing; its parent chain is never replaced.
        if let Err(e) = std::fs::create_dir_all(&dir) {
            report
                .errors
                .push(format!("cannot create {}: {e}", dir.display()));
            continue;
        }
        let real = match dir.canonicalize() {
            Ok(real) => real,
            Err(e) => {
                report
                    .errors
                    .push(format!("cannot resolve {}: {e}", dir.display()));
                continue;
            }
        };
        if !reconciled.contains(&real) {
            reconcile_tool_dir(tool, &dir, &real, &wanted, &ownership, &mut report);
            reconciled.push(real);
        }
    }
    let enabled = manifest.enabled_tools();
    for tool in TOOLS
        .iter()
        .filter(|t| !enabled.iter().any(|id| id == t.id))
    {
        let dir = tool.tool_dir(home);
        let Ok(real) = dir.canonicalize() else {
            continue;
        };
        if !reconciled.contains(&real) {
            remove_owned_entries(&dir, &real, &ownership, &mut report);
            reconciled.push(real);
        }
    }
    reconcile_instruction_locations(home, repo, manifest, &ownership, &mut report);
    report
}

/// Links the shared body to every installed tool's instruction location and
/// removes owned links from the instruction locations of known tools that are
/// not enabled. A location is compared by the realpath of its parent joined
/// with the file name (the file itself is not followed), so aliased locations
/// are reconciled once and a link from one location to another is not owned.
fn reconcile_instruction_locations(
    home: &Path,
    repo: &SkillRepo,
    manifest: &Manifest,
    ownership: &Ownership,
    report: &mut Report,
) {
    let target = repo.shared_body();
    let mut reconciled = Vec::new();
    for tool in installed_tools(home, manifest) {
        let path = home.join(tool.instruction_file);
        let location = match instruction_location(&path) {
            Ok(location) => location,
            Err(e) => {
                report
                    .errors
                    .push(format!("cannot resolve {}: {e}", path.display()));
                continue;
            }
        };
        if reconciled.contains(&location) {
            continue;
        }
        match std::fs::symlink_metadata(&location) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fsutil::symlink_atomic(&target, &location) {
                    Ok(()) => report.changes.push((Change::Linked, path)),
                    Err(e) => report
                        .errors
                        .push(format!("cannot link {}: {e}", path.display())),
                }
            }
            Err(e) => report
                .errors
                .push(format!("cannot inspect {}: {e}", path.display())),
            Ok(meta) => match ownership.owned_instruction(&location, &meta) {
                Some(current) if current == target => {}
                Some(_) => match fsutil::symlink_atomic(&target, &location) {
                    Ok(()) => report.changes.push((Change::Retargeted, path)),
                    Err(e) => report
                        .errors
                        .push(format!("cannot retarget {}: {e}", path.display())),
                },
                None => report.items.push(Item {
                    scope: location.display().to_string(),
                    path,
                    kind: ItemKind::RefusedLocation,
                }),
            },
        }
        reconciled.push(location);
    }
    let enabled = manifest.enabled_tools();
    for tool in TOOLS
        .iter()
        .filter(|t| !enabled.iter().any(|id| id == t.id))
    {
        let path = home.join(tool.instruction_file);
        let Ok(location) = instruction_location(&path) else {
            continue;
        };
        if reconciled.contains(&location) {
            continue;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&location)
            && ownership.owned_instruction(&location, &meta).is_some()
        {
            match std::fs::remove_file(&location) {
                Ok(()) => report.changes.push((Change::Removed, path)),
                Err(e) => report
                    .errors
                    .push(format!("cannot remove {}: {e}", path.display())),
            }
        }
        reconciled.push(location);
    }
}

/// The realpath of `path`'s parent joined with its file name. Fails when the
/// parent cannot be resolved; sync never creates or replaces it.
pub fn instruction_location(path: &Path) -> std::io::Result<PathBuf> {
    let parent = path.parent().unwrap_or(Path::new("/")).canonicalize()?;
    Ok(parent.join(path.file_name().unwrap_or_default()))
}

/// Tools that are enabled in `[tools].enabled` and whose presence directory exists.
pub fn installed_tools<'a>(
    home: &'a Path,
    manifest: &Manifest,
) -> impl Iterator<Item = &'static AgentTool> + 'a {
    let enabled = manifest.enabled_tools();
    TOOLS
        .iter()
        .filter(move |tool| enabled.iter().any(|id| id == tool.id) && tool.is_present(home))
}

/// One entry sync wants in every installed tool directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub name: String,
    /// The exact absolute link target: `<repo realpath>/skills/<n>` or
    /// `<repo realpath>/.skiman/managing-skills`.
    pub target: PathBuf,
}

/// Every distributable skill that is not `enabled = false`, in name order,
/// then `managing-skills`. Folders under `skills/` that are not
/// distributable are added to `report.items`.
pub fn wanted_entries(
    repo: &SkillRepo,
    manifest: &Manifest,
    report: &mut Report,
) -> std::io::Result<Vec<Wanted>> {
    let entries = manifest.skills();
    let scope = repo.skills_dir().display().to_string();
    let mut wanted = Vec::new();
    for name in repo.skill_folder_names()? {
        let dir = repo.skill_dir(&name);
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        match not_distributable(&name, &dir) {
            Some(reason) => report.items.push(Item {
                scope: scope.clone(),
                path: dir,
                kind: ItemKind::InvalidSkillFolder { reason },
            }),
            None if entries.get(&name).is_some_and(|e| !e.enabled) => {}
            None => wanted.push(Wanted { name, target: dir }),
        }
    }
    wanted.push(Wanted {
        name: managing_skills::NAME.to_owned(),
        target: repo.managing_skills_dir(),
    });
    Ok(wanted)
}

/// Why the skill folder `dir` (`skills/<name>`) is not distributable; a
/// distributable one's name passes the regex, is not reserved, and its
/// folder holds a `SKILL.md`.
fn not_distributable(name: &str, dir: &Path) -> Option<&'static str> {
    if names::is_reserved(name) {
        Some("its name is reserved")
    } else if !names::is_valid(name) {
        Some("its name is not a valid skill name")
    } else if !dir.join("SKILL.md").is_file() {
        Some("it holds no SKILL.md")
    } else {
        None
    }
}

/// The entries of `dir` with their own metadata (links not followed), sorted by name.
fn read_entries(dir: &Path, report: &mut Report) -> Vec<(OsString, std::fs::Metadata)> {
    let read = || -> std::io::Result<Vec<(OsString, std::fs::Metadata)>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            entries.push((entry.file_name(), std::fs::symlink_metadata(entry.path())?));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    };
    read().unwrap_or_else(|e| {
        report
            .errors
            .push(format!("cannot read {}: {e}", dir.display()));
        Vec::new()
    })
}

/// `dir` is the tool directory as spelled for `tool`, `real` its realpath.
fn reconcile_tool_dir(
    tool: &AgentTool,
    dir: &Path,
    real: &Path,
    wanted: &[Wanted],
    ownership: &Ownership,
    report: &mut Report,
) {
    let by_name: BTreeMap<&str, &Path> = wanted
        .iter()
        .map(|w| (w.name.as_str(), w.target.as_path()))
        .collect();
    let scope = real.display().to_string();
    let occupied = |name: &str, path: PathBuf| Item {
        scope: scope.clone(),
        path,
        kind: ItemKind::Occupied {
            name: name.to_owned(),
        },
    };
    let mut handled = BTreeSet::new();
    for (name, meta) in read_entries(dir, report) {
        let path = dir.join(&name);
        let want = name.to_str().and_then(|n| by_name.get_key_value(n));
        match (ownership.classify(real, &name, &meta), want) {
            (Entry::Owned(current), Some((name, target))) => {
                handled.insert(*name);
                if current != *target {
                    match fsutil::symlink_atomic(target, &path) {
                        Ok(()) => report.changes.push((Change::Retargeted, path)),
                        Err(e) => report
                            .errors
                            .push(format!("cannot retarget {}: {e}", path.display())),
                    }
                }
            }
            (Entry::Owned(_), None) => remove(path, report),
            (Entry::Foreign, None) => report.items.push(Item {
                scope: scope.clone(),
                path,
                kind: ItemKind::Foreign {
                    name: name.to_string_lossy().into_owned(),
                    tool: tool.id,
                },
            }),
            (_, Some((name, _))) => {
                handled.insert(*name);
                report.items.push(occupied(name, path));
            }
            (Entry::ManagedElsewhere | Entry::NotASkill, None) => {}
        }
    }
    for Wanted { name, target } in wanted {
        if handled.contains(name.as_str()) {
            continue;
        }
        let path = dir.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fsutil::symlink_atomic(target, &path) {
                    Ok(()) => report.changes.push((Change::Linked, path)),
                    Err(e) => report
                        .errors
                        .push(format!("cannot link {}: {e}", path.display())),
                }
            }
            Err(e) => report
                .errors
                .push(format!("cannot inspect {}: {e}", path.display())),
            // Another spelling of the name (APFS is case-insensitive) holds it.
            Ok(_) => report.items.push(occupied(name, path)),
        }
    }
}

/// Removes every owned entry of a tool directory nothing is distributed to.
fn remove_owned_entries(dir: &Path, real: &Path, ownership: &Ownership, report: &mut Report) {
    for (name, meta) in read_entries(dir, report) {
        if meta.is_symlink() && matches!(ownership.classify(real, &name, &meta), Entry::Owned(_)) {
            remove(dir.join(name), report);
        }
    }
}

fn remove(path: PathBuf, report: &mut Report) {
    match std::fs::remove_file(&path) {
        Ok(()) => report.changes.push((Change::Removed, path)),
        Err(e) => report
            .errors
            .push(format!("cannot remove {}: {e}", path.display())),
    }
}
