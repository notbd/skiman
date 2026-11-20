//! Distribution: the reconcile loop behind `skiman sync`, `sync --hook` and
//! the sync every write command ends with (spec Distribution and refresh).
//!
//! [`run`] is the entry point; it needs the canon lock. [`reconcile`] computes
//! and applies the changes and returns a [`Report`] of what it did and found.
//! Only owned entries and instruction locations are created, retargeted or
//! removed (spec Ownership);
//! everything else is reported as an [`Item`]. Never spawns `git`:
//! `sync --hook` must stay fast.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::failure::Failure;
use crate::fsutil;
use crate::layout::{Layout, SkillFolder, skill_folder};
use crate::linked::{self, Holder, Linked, State};
use crate::manifest::Manifest;
use crate::ownership::{Entry, Ownership};
use crate::paths::Paths;
use crate::reported::Reported;
use crate::skiman_manual;
use crate::test_hook;
use crate::tools::{AgentTool, TOOLS};
use crate::tree::WorkingTree;

/// Who reads sync's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// A command run by the user: changes on stdout, problems on stderr.
    Normal,
    /// `sync --hook`: nothing on stdout unless there is something new to report; exit 0.
    Hook,
    /// `doctor`: [`reconcile`] decides all a `Normal` sync would and
    /// writes nothing; each write it would make is a change in the report.
    Plan,
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
    Foreign {
        name: String,
        tool: &'static str,
        into: PointsInto,
    },
    /// Sync wants to link skill `name` here, but something it does not own
    /// is there (exit 3); `same` when that is a link to the same folder.
    Occupied { name: String, same: bool },
    /// A folder under `skills/` that is never distributed, and why.
    InvalidSkillFolder { reason: &'static str },
    /// An instruction location skiman does not own, so the shared body is not linked there (exit 3).
    RefusedLocation,
    /// The shared body an instruction link points at is not a readable
    /// file (`why`), so the link leads nowhere. Not recorded for the hook.
    NoSharedBody { why: String },
    /// A report on a project item or root (`crate::project::sync`): `what`
    /// names the report in the recorded set; `attention` makes it exit 3.
    Project {
        what: &'static str,
        message: String,
        attention: bool,
    },
    /// A report on a linked folder, a linked skill or `<canon>/linked`
    /// (`crate::linked`), named and judged as [`ItemKind::Project`].
    Linked {
        what: &'static str,
        message: String,
        attention: bool,
    },
}

/// Where a foreign entry leads, for the way in sync names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointsInto {
    /// Anywhere else: `skiman adopt` brings it in.
    Elsewhere,
    /// A link into a folder outside the canon that could be registered:
    /// `skiman linked add <folder>` is a way in too.
    Registrable(PathBuf),
    /// Into a registered linked folder (`id`): adopting it would copy
    /// linked content into the canon, so no adopt hint. `distributed` when
    /// the folder was read, so skiman distributes its skills; not when it
    /// is skipped or cannot be read.
    Linked { id: String, distributed: bool },
    /// Unknown while the linked folder `id` cannot be read: adopt and import
    /// refuse every entry until it can.
    Unknown(String),
}

/// The [`ItemKind::Foreign`] of the entry `name` (metadata `meta`) at
/// `path` in `tool`'s directory: a link into a linked folder gets no adopt
/// hint, nor does any entry while a linked folder cannot be read, and a
/// link into a folder that could be registered gets that hint.
pub fn foreign(
    name: &std::ffi::OsStr,
    tool: &'static str,
    path: &Path,
    meta: &std::fs::Metadata,
    repo: &Canon,
    linked: &Linked,
) -> ItemKind {
    let real = path.canonicalize().ok();
    let into = match &real {
        Some(real) if let Some(folder) = linked.reaches_into(real) => PointsInto::Linked {
            id: folder.id.clone(),
            distributed: matches!(folder.state, State::Read(_)),
        },
        _ if let Some(folder) = linked.unreadable() => PointsInto::Unknown(folder.id.clone()),
        Some(real)
            if meta.is_symlink()
                && !real.starts_with(repo.root())
                && let Some(parent) = real.parent()
                && linked.admits(parent) =>
        {
            PointsInto::Registrable(parent.to_owned())
        }
        _ => PointsInto::Elsewhere,
    };
    ItemKind::Foreign {
        name: name.to_string_lossy().into_owned(),
        tool,
        into,
    }
}

/// The [`ItemKind::Occupied`] of `path`, which holds the name of the skill
/// `name` whose link would point at `target`.
pub fn occupied(name: &str, path: &Path, target: &Path) -> ItemKind {
    let same = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_symlink())
        && path
            .canonicalize()
            .is_ok_and(|real| target.canonicalize().is_ok_and(|t| t == real));
    ItemKind::Occupied {
        name: name.to_owned(),
        same,
    }
}

impl Item {
    /// What identifies the item in the recorded set.
    pub fn id(&self) -> String {
        let kind = match self.kind {
            ItemKind::Foreign { .. } => "foreign",
            ItemKind::Occupied { .. } => "occupied",
            ItemKind::InvalidSkillFolder { .. } => "invalid-skill-folder",
            ItemKind::RefusedLocation => "refused-location",
            ItemKind::NoSharedBody { .. } => "no-shared-body",
            ItemKind::Project { what, .. } => {
                return format!("project-{what} {}", self.path.display());
            }
            ItemKind::Linked { what, .. } => {
                return format!("linked-{what} {}", self.path.display());
            }
        };
        format!("{kind} {}", self.path.display())
    }

    pub fn message(&self) -> String {
        let path = self.path.display();
        match &self.kind {
            ItemKind::Foreign {
                name,
                tool,
                into: PointsInto::Elsewhere,
            } => format!(
                "foreign skill {path} is not managed by skiman; bring it in with `skiman adopt {name} --from {tool}`"
            ),
            ItemKind::Foreign {
                name,
                tool,
                into: PointsInto::Registrable(folder),
            } => format!(
                "foreign skill {path} is not managed by skiman; bring it in with `skiman adopt {name} --from {tool}`, or register the folder it is in with `skiman linked add {}`",
                crate::project::shell_quote(&folder.display().to_string())
            ),
            ItemKind::Foreign {
                into: PointsInto::Unknown(id),
                ..
            } => format!(
                "foreign skill {path} is not managed by skiman; it can be brought in once linked folder `{id}` can be read"
            ),
            ItemKind::Foreign {
                into: PointsInto::Linked { id, distributed },
                ..
            } => {
                let hint = if *distributed {
                    ", which skiman already distributes; delete it once nothing needs it"
                } else {
                    ""
                };
                format!("{path} is a link into linked folder `{id}`{hint}")
            }
            ItemKind::Occupied { name, same: true } => format!(
                "{path} is a link to the same folder as skill {name}, but skiman did not make it; deleting it is safe: rm {}",
                crate::project::shell_quote(&path.to_string())
            ),
            ItemKind::Occupied { name, same: false } => format!(
                "{path} is occupied by an entry skiman does not own, so skill {name} is not linked there; move it away, then run `skiman sync`"
            ),
            ItemKind::InvalidSkillFolder { reason } => {
                format!("skill folder {path} is not distributed: {reason}")
            }
            ItemKind::RefusedLocation => format!(
                "instruction file {path} is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`"
            ),
            ItemKind::NoSharedBody { why } => format!(
                "the shared body {path} {why}, so every instruction file linked to it leads nowhere; make it a file (it may be empty)"
            ),
            ItemKind::Project { message, .. } | ItemKind::Linked { message, .. } => message.clone(),
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
    /// For [`Output::Plan`]: the tool-directory entries sync leaves alone
    /// without a report (not owned, foreign or in the way).
    pub left: Vec<(PathBuf, Entry)>,
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
                    | ItemKind::Linked {
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
pub fn run(paths: &Paths, repo: &Canon, lock: &CanonLock, output: Output) -> Exit {
    run_with(paths, repo, lock, output, None, &[]).0
}

/// The canon `init` just switched away from.
#[derive(Debug)]
pub struct PreviousCanon {
    /// Its realpath: skiman's links into it count as owned for one sync.
    pub root: PathBuf,
}

/// [`run`], also returning the report, after `init` switched away from
/// `previous`. Nothing is said about `failed`, paths whose write the
/// command just reported failing: the way out sync names for them is that
/// same write.
pub fn run_with(
    paths: &Paths,
    repo: &Canon,
    _lock: &CanonLock,
    output: Output,
    previous: Option<&PreviousCanon>,
    failed: &[PathBuf],
) -> (Exit, Report) {
    // A plan writes nothing; this function records what it reported.
    assert_ne!(
        output,
        Output::Plan,
        "doctor runs reconcile, never run_with"
    );
    let mut report = match Manifest::load(repo) {
        Ok(manifest) => {
            let linked = Linked::read(paths.home(), repo, &manifest);
            let previous_canon = previous.map(|p| p.root.clone());
            let mut report = reconcile(paths, repo, &manifest, &linked, previous_canon, output);
            let records = (&manifest, &linked, None);
            crate::project::sync::reconcile(paths, repo, records, output, &mut report);
            report
        }
        Err(failure) => return (report_failure(failure, output), Report::default()),
    };
    report.items.retain(|item| !failed.contains(&item.path));
    if output == Output::Normal {
        for (change, entry) in &report.changes {
            let verb = match change {
                Change::Linked => "linked",
                Change::Retargeted => "retargeted",
                Change::Removed => "removed",
                Change::Mirrored => "mirrored",
            };
            out!("{verb} {}", entry.display());
        }
    }
    let last = Reported::load(paths);
    let mut now = Reported::default();
    for item in &report.items {
        let id = item.id();
        match output {
            Output::Normal | Output::Plan => err!("warning: {}", item.message()),
            Output::Hook if !last.contains(&item.scope, &id) => {
                out!("skiman: {}", item.message())
            }
            Output::Hook => {}
        }
        // Only `skiman sync` looks for it: recorded, the hook would drop it each time.
        if !matches!(item.kind, ItemKind::NoSharedBody { .. }) {
            now.insert(&item.scope, id);
        }
    }
    if now != last
        && let Err(e) = now.save(paths)
    {
        report
            .errors
            .push(format!("cannot write {}: {e}", paths.state_dir().display()));
    }
    for error in &report.errors {
        err!("error: {error}");
    }
    let exit = match output {
        Output::Hook => Exit::Success,
        _ => report.exit(),
    };
    (exit, report)
}

/// Prints a failure that stops sync before it reconciles anything.
pub fn report_failure(failure: Failure, output: Output) -> Exit {
    let exit = failure.report();
    match output {
        Output::Hook => Exit::Success,
        _ => exit,
    }
}

/// Materialises `skiman-manual` and links each linked folder into the
/// canon, then makes every installed tool's tool directory hold an owned
/// link for every wanted entry, removes owned entries nothing wants, and
/// removes owned entries from tool directories of known tools that are not
/// enabled. Then does the same for instruction locations and the shared body.
/// For [`Output::Plan`] it writes nothing (see [`make`]).
pub fn reconcile(
    paths: &Paths,
    repo: &Canon,
    manifest: &Manifest,
    linked: &Linked,
    previous_canon: Option<PathBuf>,
    output: Output,
) -> Report {
    let home = paths.home();
    let mut report = Report::default();
    reconcile_manual(repo, output, &mut report);
    let kept = kept_names(paths, repo, manifest, linked);
    linked::canon::report_folders(linked, &mut report);
    let offered = linked::canon::reconcile(linked, output, &mut report);
    let ownership = Ownership::new(paths, repo, previous_canon);
    let offers = |id: &str| offered.contains(id);
    let wanted = match wanted_entries(repo, manifest, linked, &kept, &offers, &mut report) {
        Ok(wanted) => wanted,
        Err(e) => {
            report
                .errors
                .push(format!("cannot read {}: {e}", repo.skills_dir().display()));
            return report;
        }
    };
    let targets = write_targets(home, manifest);
    // Tool directories that resolve to the same real directory are reconciled once.
    let mut reconciled = Vec::new();
    // Those of them the guard refused: sync never makes them.
    let mut refused = Vec::new();
    for target in targets
        .iter()
        .filter(|t| t.place == Place::ToolDir && t.links)
    {
        let (tool, dir, lands) = (target.tool, &target.path, &target.lands);
        if reconciled.contains(lands) {
            continue;
        }
        if guarded(linked, dir, lands, &mut report) {
            reconciled.push(lands.clone());
            refused.push(lands.clone());
            continue;
        }
        let plan = output == Output::Plan;
        // The tool directory itself may be missing; its parent chain is never replaced.
        let (missing, through) = match missing_dir(lands) {
            Ok(missing) => (missing, lands.clone()),
            // Links on the way that lead nowhere until sync makes a folder for
            // an earlier tool: sync makes that folder first, so this is that
            // folder, or one it can make once every broken link leads somewhere.
            Err(why) => match plan.then(|| followed(lands, &reconciled, &refused)) {
                Some(Some(to)) if reconciled.contains(&to) => continue,
                Some(Some(to)) => (true, to),
                _ => {
                    let dir = dir.display();
                    report
                        .errors
                        .push(format!("cannot use {dir} as a tool directory: {why}"));
                    continue;
                }
            },
        };
        // Created and written where the guard checked, never through `dir`
        // again: a link on its way may change meanwhile.
        if missing && !plan {
            test_hook::at("sync", &format!("create:{}", dir.display()));
            if let Err(e) = std::fs::create_dir_all(&through) {
                report
                    .errors
                    .push(format!("cannot create {}: {e}", dir.display()));
                continue;
            }
        }
        let real = match through.canonicalize() {
            Ok(real) => real,
            // A plan goes on as if the missing folder were made, empty.
            Err(_) if missing && plan => through.clone(),
            Err(e) => {
                report
                    .errors
                    .push(format!("cannot resolve {}: {e}", dir.display()));
                continue;
            }
        };
        // A folder created as a link meanwhile, or one a plan reached past a
        // broken link, is checked where it leads.
        if real != *lands && guarded(linked, dir, &real, &mut report) {
            refused.push(real.clone());
            reconciled.push(real);
            continue;
        }
        if !reconciled.contains(&real) {
            let tool_dir = ToolDir {
                tool,
                dir,
                real: &real,
                // Only a plan takes the folder as empty: one that was just made is read.
                missing: missing && plan,
            };
            let names = Names {
                wanted: &wanted,
                kept: &kept,
                output,
            };
            reconcile_tool_dir(&tool_dir, repo, linked, &names, &ownership, &mut report);
            reconciled.push(real);
        }
    }
    for target in targets
        .iter()
        .filter(|t| t.place == Place::ToolDir && !t.links)
    {
        let (dir, real) = (&target.path, &target.lands);
        if !reconciled.contains(real) && !guarded(linked, dir, real, &mut report) {
            let tool_dir = ToolDir {
                tool: target.tool,
                dir,
                real,
                missing: false,
            };
            remove_owned_entries(&tool_dir, (&ownership, output), &mut report);
        }
        reconciled.push(real.clone());
    }
    let context = (repo, linked, &ownership);
    reconcile_instruction_locations(&targets, context, output, &mut report);
    report
}

/// Writes `skiman-manual` when it is missing or out of date, once
/// [`skiman_manual::needs_write`] passes, and first removes the manual's
/// folder from before its rename (derived content, so no backup; a link
/// there is removed, not followed). A check or removal that fails leaves
/// the manual as it is.
fn reconcile_manual(repo: &Canon, output: Output, report: &mut Report) {
    let cannot = |e: std::io::Error| format!("cannot write {}: {e}", repo.manual_dir().display());
    let write = match skiman_manual::needs_write(repo) {
        Ok(write) => write,
        Err(e) => return report.errors.push(cannot(e)),
    };
    let retired = repo.retired_manual_dir();
    match std::fs::symlink_metadata(&retired) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            let retired = retired.display();
            return report.errors.push(format!("cannot remove {retired}: {e}"));
        }
        Ok(meta) => {
            let remove = || {
                let gone = if meta.is_dir() {
                    std::fs::remove_dir_all(&retired)
                } else {
                    std::fs::remove_file(&retired)
                };
                gone.map(|()| true)
            };
            if make(output, report, (Change::Removed, &retired), remove).is_none() {
                return;
            }
        }
    }
    if write
        && output != Output::Plan
        && let Err(e) = skiman_manual::materialise(repo)
    {
        report.errors.push(cannot(e));
    }
}

/// Makes one write of a reconcile with `write` and records `change` at
/// `path`; for [`Output::Plan`] only records it. `Some(false)`: something
/// else took `path` first and stays. `None`: the write failed (reported).
pub fn make(
    output: Output,
    report: &mut Report,
    (change, path): (Change, &Path),
    write: impl FnOnce() -> std::io::Result<bool>,
) -> Option<bool> {
    let made = match output {
        Output::Plan => true,
        _ => write()
            .map_err(|e| {
                let verb = match change {
                    Change::Linked => "link",
                    Change::Retargeted => "retarget",
                    _ => "remove",
                };
                let path = path.display();
                report.errors.push(format!("cannot {verb} {path}: {e}"));
            })
            .ok()?,
    };
    if made {
        report.changes.push((change, path.to_owned()));
    }
    Some(made)
}

/// A tool directory or an instruction location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    ToolDir,
    Instructions,
}

/// A place outside the canon that sync writes in (spec "Write targets never
/// reach into a linked folder").
#[derive(Debug)]
pub struct WriteTarget {
    pub tool: &'static AgentTool,
    pub place: Place,
    /// The tool is installed and enabled, so sync links there; otherwise
    /// sync only removes its own links.
    pub links: bool,
    /// As the tool names it.
    pub path: PathBuf,
    /// Where a write there lands, as the write guard compares it.
    pub lands: PathBuf,
}

/// Every place sync writes in under `manifest`, in the order it writes
/// them: each installed tool's directory, each other known tool's that
/// exists, then the same for instruction locations (one whose parent does
/// not resolve is left out for a tool that is not enabled). Sync and
/// import's check before any write both use this list.
pub fn write_targets(home: &Path, manifest: &Manifest) -> Vec<WriteTarget> {
    let enabled = manifest.enabled_tools();
    let installed: Vec<&'static AgentTool> = installed_tools(home, manifest).collect();
    let others = || {
        TOOLS
            .iter()
            .filter(|t| !enabled.iter().any(|id| id == t.id))
    };
    let target = |tool, place, links, path, lands| WriteTarget {
        tool,
        place,
        links,
        path,
        lands,
    };
    let mut targets = Vec::new();
    for &tool in &installed {
        let dir = tool.tool_dir(home);
        let lands = linked::written_through(&dir);
        targets.push(target(tool, Place::ToolDir, true, dir, lands));
    }
    for tool in others() {
        let dir = tool.tool_dir(home);
        if let Ok(real) = dir.canonicalize() {
            targets.push(target(tool, Place::ToolDir, false, dir, real));
        }
    }
    for &tool in &installed {
        let path = home.join(tool.instruction_file);
        let lands = linked::location(&path);
        targets.push(target(tool, Place::Instructions, true, path, lands));
    }
    for tool in others() {
        let path = home.join(tool.instruction_file);
        if let Ok(location) = instruction_location(&path) {
            targets.push(target(tool, Place::Instructions, false, path, location));
        }
    }
    targets
}

/// Links the shared body to every installed tool's instruction location and
/// removes owned links from the instruction locations of known tools that are
/// not enabled. A location is compared by the realpath of its parent joined
/// with the file name (the file itself is not followed), so aliased locations
/// are reconciled once and a link from one location to another is not owned.
fn reconcile_instruction_locations(
    targets: &[WriteTarget],
    (repo, linked, ownership): (&Canon, &Linked, &Ownership),
    output: Output,
    report: &mut Report,
) {
    let target = repo.shared_body();
    let mut reconciled = Vec::new();
    // Whether an installed tool's location is, or is to be, a link to the body.
    let mut leads_to_body = false;
    // Locations inside a linked folder: never written, never swept.
    let mut refused = Vec::new();
    for to in targets
        .iter()
        .filter(|t| t.place == Place::Instructions && t.links)
    {
        let (path, lands) = (to.path.clone(), &to.lands);
        if refused.contains(lands) || guarded(linked, &path, lands, report) {
            refused.push(lands.clone());
            continue;
        }
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
        // Resolved again here, so checked again where it is written.
        if location != *lands && guarded(linked, &path, &location, report) {
            refused.push(location);
            continue;
        }
        let refused = |path: PathBuf| Item {
            scope: location.display().to_string(),
            path,
            kind: ItemKind::RefusedLocation,
        };
        let real = location.parent().expect("named");
        let taken = match std::fs::symlink_metadata(&location) {
            // `symlink` never replaces: a file that appeared since the check
            // is refused like one the check found.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let link = || fsutil::link_new(&target, &location, &location);
                let made = make(output, report, (Change::Linked, &path), link);
                leads_to_body |= made == Some(true);
                made == Some(false)
            }
            Err(e) => {
                let path = path.display();
                report.errors.push(format!("cannot inspect {path}: {e}"));
                false
            }
            Ok(meta) => match ownership.owned_instruction(&location, &meta) {
                Some(current) if current == target => {
                    leads_to_body = true;
                    false
                }
                Some(current) => {
                    let relink = || fsutil::relink(&location, real, &current, Some(&target));
                    let made = make(output, report, (Change::Retargeted, &path), relink);
                    leads_to_body |= made == Some(true);
                    made == Some(false)
                }
                None => true,
            },
        };
        if taken {
            report.items.push(refused(path));
        }
        reconciled.push(location);
    }
    for to in targets
        .iter()
        .filter(|t| t.place == Place::Instructions && !t.links)
    {
        let (path, location) = (to.path.clone(), to.lands.clone());
        if reconciled.contains(&location) || refused.contains(&location) {
            continue;
        }
        if guarded(linked, &path, &location, report) {
            refused.push(location);
            continue;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&location)
            && let Some(current) = ownership.owned_instruction(&location, &meta)
        {
            let real = location.parent().expect("named");
            remove(output, (&location, real, &current), &path, report);
        }
        reconciled.push(location);
    }
    // Only a sync that may read more than it changes looks for these.
    if output == Output::Hook {
        return;
    }
    let why = || match std::fs::metadata(&target) {
        Ok(meta) if meta.is_file() => None,
        Ok(_) => Some("is not a file".to_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some("is missing".to_owned()),
        Err(e) => Some(format!("cannot be read ({e})")),
    };
    if leads_to_body && let Some(why) = why() {
        let (scope, path) = (target.display().to_string(), target.clone());
        let kind = ItemKind::NoSharedBody { why };
        report.items.push(Item { scope, path, kind });
    }
    // A sync killed while it took one of these links aside left it under a
    // temporary name.
    for location in &reconciled {
        let dir = location.parent().expect("named");
        let name = location.file_name().expect("named");
        // Names only: the folder is the tool's own, and its files come and go.
        for aside in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = aside.path();
            if fsutil::temporary_of(&aside.file_name()) == Some(name)
                && let Ok(meta) = std::fs::symlink_metadata(&path)
                && let Some(current) = ownership.owned_instruction(&path, &meta)
            {
                remove(output, (&path, dir, &current), &path, report);
            }
        }
    }
}

/// Reports `path` and returns `true` when it resolves to `target` inside a
/// linked folder: skiman writes nothing there (exit 3).
fn guarded(linked: &Linked, path: &Path, target: &Path, report: &mut Report) -> bool {
    let Some(message) = linked.refusal(path, target) else {
        return false;
    };
    report.items.push(Item {
        scope: target.display().to_string(),
        path: path.to_owned(),
        kind: ItemKind::Linked {
            what: "guarded",
            message,
            attention: true,
        },
    });
    true
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

/// The names that links [`Linked::keeper`] keeps in the installed tool
/// directories, with what keeps each: a transient read error never hands
/// a name to another skill. Reads the tool directories only while some
/// link may be kept. Sync and every command that shows a winner use it.
pub fn kept_names(
    paths: &Paths,
    repo: &Canon,
    manifest: &Manifest,
    linked: &Linked,
) -> BTreeMap<String, Holder> {
    let mut kept = BTreeMap::new();
    if !linked.keeps_any() {
        return kept;
    }
    let home = paths.home();
    let ownership = Ownership::new(paths, repo, None);
    for tool in installed_tools(home, manifest) {
        let dir = tool.tool_dir(home);
        let Ok(real) = dir.canonicalize() else {
            continue;
        };
        for (name, meta) in read_entries(&real, &dir, &mut Report::default()) {
            if let Entry::Owned(target) = ownership.classify(&real, &name, &meta)
                && let Some(holder) = linked.keeper(&target)
            {
                let name = fsutil::temporary_of(&name).unwrap_or(&name);
                let name = name.to_string_lossy().into_owned();
                kept.insert(name, holder);
            }
        }
    }
    kept
}

/// One entry sync wants in every installed tool directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub name: String,
    /// The exact absolute link target: `<repo realpath>/skills/<n>`,
    /// `<repo realpath>/skills/<group>/<n>`,
    /// `<repo realpath>/.skiman/skiman-manual`, or for a linked skill
    /// `<repo realpath>/linked/<id>/<n>` or `<repo realpath>/linked/<id>`.
    pub target: PathBuf,
}

/// Every distributable skill that is not `enabled = false` and whose name
/// no kept link holds ([`linked::withheld`]), in name order, then
/// `skiman-manual`, then the linked skills of the folders `offers` accepts
/// that no other skill holds the name of ([`Linked::names`]; `kept` from
/// [`kept_names`]). Folders under `skills/` that are not distributable or
/// are withheld, and linked skills that lose their name, are added to
/// `report.items`.
pub fn wanted_entries(
    repo: &Canon,
    manifest: &Manifest,
    linked: &Linked,
    kept: &BTreeMap<String, Holder>,
    offers: &dyn Fn(&str) -> bool,
    report: &mut Report,
) -> std::io::Result<Vec<Wanted>> {
    let entries = manifest.skills();
    let scope = repo.skills_dir().display().to_string();
    let layout = Layout::read(&WorkingTree::new(repo.root()))?;
    let withheld = linked::withheld(repo.root(), &layout, manifest, kept);
    let mut wanted = Vec::new();
    for placed in layout.skills() {
        let (name, dir) = (&placed.name, repo.root().join(placed.path()));
        match skill_folder(&layout, placed, &dir) {
            SkillFolder::NotAFolder => {}
            SkillFolder::Invalid(reason) => report.items.push(Item {
                scope: scope.clone(),
                path: dir,
                kind: ItemKind::InvalidSkillFolder { reason },
            }),
            SkillFolder::Distributable if entries.get(name).is_some_and(|e| !e.enabled) => {}
            SkillFolder::Distributable
                if let Some(withheld) = withheld.iter().find(|w| w.path == placed.path()) =>
            {
                report.items.push(Item {
                    scope: scope.clone(),
                    kind: ItemKind::Linked {
                        what: "collision",
                        message: withheld.message(),
                        attention: true,
                    },
                    path: dir,
                })
            }
            SkillFolder::Distributable => wanted.push(Wanted {
                name: name.clone(),
                target: dir,
            }),
        }
    }
    wanted.push(Wanted {
        name: skiman_manual::NAME.to_owned(),
        target: repo.manual_dir(),
    });
    let (offered, collisions) = linked.names(&layout, kept, offers);
    wanted.extend(
        offered
            .into_iter()
            .map(|(name, target)| Wanted { name, target }),
    );
    for collision in collisions {
        report.items.push(Item {
            scope: linked.dir().display().to_string(),
            path: collision.dir.clone(),
            kind: ItemKind::Linked {
                what: "collision",
                message: collision.message(),
                attention: true,
            },
        });
    }
    Ok(wanted)
}

/// The entries of `dir` with their own metadata (links not followed),
/// sorted by name; a failure is reported as reading `shown`.
fn read_entries(
    dir: &Path,
    shown: &Path,
    report: &mut Report,
) -> Vec<(OsString, std::fs::Metadata)> {
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
            .push(format!("cannot read {}: {e}", shown.display()));
        Vec::new()
    })
}

/// A tool directory of `tool`: `dir` as spelled, which messages show, and
/// `real` its realpath as the write guard checked it, which every read
/// and write goes through; `missing` when a plan takes it as made and empty.
struct ToolDir<'a> {
    tool: &'a AgentTool,
    dir: &'a Path,
    real: &'a Path,
    missing: bool,
}

/// The same for every tool directory in one sync: the entries it wants
/// ([`wanted_entries`]), the names kept links hold ([`kept_names`]), and
/// whether it writes ([`Output::Plan`] does not).
struct Names<'a> {
    wanted: &'a [Wanted],
    kept: &'a BTreeMap<String, Holder>,
    output: Output,
}

/// Links every wanted entry in the tool directory and removes the owned
/// entries nothing wants; owned links [`Linked::keeper`] keeps, and any
/// owned link under a name one of them holds elsewhere (`kept`), stay
/// exactly as they are and keep their name.
fn reconcile_tool_dir(
    tool_dir: &ToolDir,
    repo: &Canon,
    linked: &Linked,
    names: &Names,
    ownership: &Ownership,
    report: &mut Report,
) {
    let ToolDir {
        tool,
        dir,
        real,
        missing,
    } = *tool_dir;
    let Names {
        wanted,
        kept,
        output,
    } = *names;
    let by_name: BTreeMap<&str, &Path> = wanted
        .iter()
        .map(|w| (w.name.as_str(), w.target.as_path()))
        .collect();
    let scope = real.display().to_string();
    let occupied = |name: &str, at: &Path, path: PathBuf| Item {
        scope: scope.clone(),
        kind: occupied(name, at, by_name[name]),
        path,
    };
    let mut handled = BTreeSet::new();
    let entries = if missing {
        Vec::new()
    } else {
        read_entries(real, dir, report)
    };
    for (name, meta) in entries {
        let (at, path) = (real.join(&name), dir.join(&name));
        let want = name.to_str().and_then(|n| by_name.get_key_value(n));
        let entry = ownership.classify(real, &name, &meta);
        if let Entry::Owned(current) = &entry
            && (linked.keeper(current).is_some()
                || name.to_str().is_some_and(|n| kept.contains_key(n)))
        {
            if let Some((name, _)) = want {
                handled.insert(*name);
            }
            continue;
        }
        match (entry, want) {
            (Entry::Owned(current), Some((name, target))) => {
                handled.insert(*name);
                let relink = || fsutil::relink(&at, real, &current, Some(target));
                if current != *target
                    && make(output, report, (Change::Retargeted, &path), relink) == Some(false)
                {
                    report.items.push(occupied(name, &at, path));
                }
            }
            (Entry::Owned(current), None) => {
                remove(output, (&at, real, &current), &path, report);
            }
            (Entry::Foreign, None) => report.items.push(Item {
                scope: scope.clone(),
                kind: foreign(&name, tool.id, &at, &meta, repo, linked),
                path,
            }),
            (entry, Some((name, _))) => {
                handled.insert(*name);
                if output == Output::Plan && matches!(entry, Entry::Moved(_)) {
                    report.left.push((path.clone(), entry));
                }
                report.items.push(occupied(name, &at, path));
            }
            (entry, None) if output == Output::Plan => report.left.push((path, entry)),
            (Entry::ManagedElsewhere | Entry::NotASkill | Entry::Moved(_), None) => {}
        }
    }
    for Wanted { name, target } in wanted {
        if handled.contains(name.as_str()) {
            continue;
        }
        let (at, path) = (real.join(name), dir.join(name));
        // Another spelling of the name (APFS is case-insensitive), or an
        // entry that appeared since the check, occupies it like one the check found.
        if link_if_free((output, report), (&at, &path, target)) == Some(false) {
            report.items.push(occupied(name, &at, path));
        }
    }
}

/// Makes the link `at` (shown as `path`) to `target` through [`make`] when
/// nothing holds the name; `Some(false)` when something does (another
/// spelling of it on a case-insensitive volume counts), `None` on an error
/// it reports.
pub fn link_if_free(
    (output, report): (Output, &mut Report),
    (at, path, target): (&Path, &Path, &Path),
) -> Option<bool> {
    match std::fs::symlink_metadata(at) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let link = || fsutil::link_new(target, at, path);
            make(output, report, (Change::Linked, path), link)
        }
        Err(e) => {
            let path = path.display();
            report.errors.push(format!("cannot inspect {path}: {e}"));
            None
        }
        Ok(_) => Some(false),
    }
}

/// Removes every owned entry of a tool directory nothing is distributed to.
fn remove_owned_entries(
    tool_dir: &ToolDir,
    (ownership, output): (&Ownership, Output),
    report: &mut Report,
) {
    let ToolDir { dir, real, .. } = *tool_dir;
    for (name, meta) in read_entries(real, dir, report) {
        if let (true, Entry::Owned(current)) =
            (meta.is_symlink(), ownership.classify(real, &name, &meta))
        {
            let (at, path) = (real.join(&name), dir.join(&name));
            remove(output, (&at, real, &current), &path, report);
        }
    }
}

/// Removes skiman's own link at `link` (in `real_dir`, pointing at
/// `current`) through [`make`], reported as `path`; one that became
/// something else stays.
fn remove(
    output: Output,
    (link, real_dir, current): (&Path, &Path, &Path),
    path: &Path,
    report: &mut Report,
) {
    let relink = || fsutil::relink(link, real_dir, current, None);
    make(output, report, (Change::Removed, path), relink);
}

/// Where `dir` lands once every link on its way that leads nowhere is mended
/// by a folder in `reconciled` that sync makes for an earlier tool (not one
/// of `refused`, which it never makes); `None` when every link leads
/// somewhere (or cannot be told), or any broken link still leads nowhere
/// afterwards.
fn followed(dir: &Path, reconciled: &[PathBuf], refused: &[PathBuf]) -> Option<PathBuf> {
    let resolved = linked::path::resolve(dir);
    let broken: Vec<_> = resolved
        .links
        .iter()
        .filter(|link| matches!(link.try_exists(), Ok(false)))
        .collect();
    if broken.is_empty() {
        return None;
    }
    let made = |to: &Path| {
        let mut made = reconciled.iter().filter(|d| !refused.contains(d));
        made.any(|made| made.starts_with(to))
    };
    let mended = |link: &&PathBuf| {
        std::fs::read_link(link)
            .ok()
            .is_some_and(|target| made(&linked::path::resolve(&link.with_file_name(target)).path))
    };
    broken.iter().all(mended).then_some(resolved.path)
}

/// Whether the tool directory `dir` is missing and can be made (`true`) or
/// is a folder (`false`); otherwise why it cannot be used, judged before
/// anything is written: it, or the first parent that is there, is a link
/// that leads nowhere or round in a circle, is not a folder, or cannot be read.
pub fn missing_dir(dir: &Path) -> Result<bool, String> {
    // Whether `path` is a folder (`true`) or nothing (`false`).
    let folder = |path: &Path| match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
        // Not a link: what lstat said is what it is.
        Ok(meta) if !meta.is_symlink() && meta.is_dir() => Ok(true),
        Ok(meta) if !meta.is_symlink() => Err(format!("{} is not a folder", path.display())),
        Ok(_) => match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => Ok(true),
            Ok(_) => Err(format!("{} is not a folder", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(format!("{} is a link that leads nowhere", path.display()))
            }
            Err(e) => Err(e.to_string()),
        },
    };
    if folder(dir)? {
        return Ok(false);
    }
    for parent in dir.ancestors().skip(1) {
        if folder(parent)? {
            break;
        }
    }
    Ok(true)
}
