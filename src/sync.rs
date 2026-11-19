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
use crate::layout::Layout;
use crate::linked::{self, Linked, State};
use crate::manifest::Manifest;
use crate::names;
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
}

/// The [`ItemKind::Foreign`] of the entry `name` (metadata `meta`) at
/// `path` in `tool`'s directory: a link into a linked folder gets no adopt
/// hint, and a link into a folder that could be registered gets that hint.
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

/// [`run`], also returning the report. `previous_canon` is the realpath of the
/// canon `init` just switched away from: its links count as owned.
/// Nothing is said about `failed`, paths whose write the command just
/// reported failing: the way out sync names for them is that same write.
pub fn run_with(
    paths: &Paths,
    repo: &Canon,
    _lock: &CanonLock,
    output: Output,
    previous_canon: Option<PathBuf>,
    failed: &[PathBuf],
) -> (Exit, Report) {
    let mut report = match Manifest::load(repo) {
        Ok(manifest) => {
            let linked = Linked::read(paths.home(), repo, &manifest);
            let mut report = reconcile(paths, repo, &manifest, &linked, previous_canon, output);
            crate::project::sync::reconcile(paths, repo, &manifest, &linked, &mut report);
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
            Output::Normal => err!("warning: {}", item.message()),
            Output::Hook if !last.contains(&item.scope, &id) => {
                out!("skiman: {}", item.message())
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
        err!("error: {error}");
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

/// Materialises `skiman-manual` and links each linked folder into the
/// canon, then makes every installed tool's tool directory hold an owned
/// link for every wanted entry, removes owned entries nothing wants, and
/// removes owned entries from tool directories of known tools that are not
/// enabled. Then does the same for instruction locations and the shared body.
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
    if let Err(e) = skiman_manual::materialise(repo) {
        report
            .errors
            .push(format!("cannot write {}: {e}", repo.manual_dir().display()));
    }
    linked::canon::report_folders(linked, &mut report);
    let offered = linked::canon::reconcile(linked, &mut report);
    let ownership = Ownership::new(paths, repo, previous_canon);
    let kept = kept_names(home, manifest, linked, &ownership);
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
    let frozen = linked.frozen();
    // Tool directories that resolve to the same real directory are reconciled once.
    let mut reconciled = Vec::new();
    for tool in installed_tools(home, manifest) {
        let dir = tool.tool_dir(home);
        let through = linked::written_through(&dir);
        if reconciled.contains(&through) || guarded(linked, &dir, &through, &mut report) {
            reconciled.push(through);
            continue;
        }
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
            let tool_dir = ToolDir {
                tool,
                dir: &dir,
                real: &real,
            };
            reconcile_tool_dir(
                &tool_dir,
                repo,
                linked,
                &wanted,
                &frozen,
                &ownership,
                &mut report,
            );
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
        if !reconciled.contains(&real) && !guarded(linked, &dir, &real, &mut report) {
            remove_owned_entries(&dir, &real, &ownership, &mut report);
        }
        reconciled.push(real);
    }
    let sweep = output == Output::Normal;
    let context = (repo, manifest, linked, &ownership);
    reconcile_instruction_locations(home, context, sweep, &mut report);
    report
}

/// Links the shared body to every installed tool's instruction location and
/// removes owned links from the instruction locations of known tools that are
/// not enabled. A location is compared by the realpath of its parent joined
/// with the file name (the file itself is not followed), so aliased locations
/// are reconciled once and a link from one location to another is not owned.
fn reconcile_instruction_locations(
    home: &Path,
    (repo, manifest, linked, ownership): (&Canon, &Manifest, &Linked, &Ownership),
    sweep: bool,
    report: &mut Report,
) {
    let target = repo.shared_body();
    let mut reconciled = Vec::new();
    // Locations inside a linked folder: never written, never swept.
    let mut refused = Vec::new();
    for tool in installed_tools(home, manifest) {
        let path = home.join(tool.instruction_file);
        let lands = linked::location(&path);
        if refused.contains(&lands) || guarded(linked, &path, &lands, report) {
            refused.push(lands);
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
        let refused = |path: PathBuf| Item {
            scope: location.display().to_string(),
            path,
            kind: ItemKind::RefusedLocation,
        };
        match std::fs::symlink_metadata(&location) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                test_hook::at("sync", &format!("link:{}", location.display()));
                // `symlink` never replaces: a file that appeared since the
                // check is refused like one the check found.
                match std::os::unix::fs::symlink(&target, &location) {
                    Ok(()) => report.changes.push((Change::Linked, path)),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        report.items.push(refused(path))
                    }
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
                Some(current) => {
                    let real = location.parent().expect("named");
                    match fsutil::relink(&location, real, &current, Some(&target)) {
                        Ok(true) => report.changes.push((Change::Retargeted, path)),
                        Ok(false) => report.items.push(refused(path)),
                        Err(e) => report
                            .errors
                            .push(format!("cannot retarget {}: {e}", path.display())),
                    }
                }
                None => report.items.push(refused(path)),
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
            remove(&location, real, &current, path, report);
        }
        reconciled.push(location);
    }
    // A sync killed while it took one of these links aside left it under a
    // temporary name; only a sync that may read more than it changes looks.
    for location in reconciled.iter().filter(|_| sweep) {
        let dir = location.parent().expect("named");
        let name = location.file_name().expect("named");
        // Names only: the folder is the tool's own, and its files come and go.
        for aside in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = aside.path();
            if fsutil::temporary_of(&aside.file_name()) == Some(name)
                && let Ok(meta) = std::fs::symlink_metadata(&path)
                && let Some(current) = ownership.owned_instruction(&path, &meta)
            {
                remove(&path, dir, &current, path.clone(), report);
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

/// The names that links of unreadable linked folders keep in the installed
/// tool directories, with the folder's id: a transient read error never
/// hands a name to another skill. Reads the tool directories only while a
/// folder cannot be read.
fn kept_names(
    home: &Path,
    manifest: &Manifest,
    linked: &Linked,
    ownership: &Ownership,
) -> BTreeMap<String, String> {
    let mut kept = BTreeMap::new();
    if !linked
        .folders
        .iter()
        .any(|f| matches!(f.state, linked::State::Unreadable(_)))
    {
        return kept;
    }
    for tool in installed_tools(home, manifest) {
        let dir = tool.tool_dir(home);
        let Ok(real) = dir.canonicalize() else {
            continue;
        };
        for (name, meta) in read_entries(&dir, &mut Report::default()) {
            if let Entry::Owned(target) = ownership.classify(&real, &name, &meta)
                && let Some(id) = linked.keeper(&target)
            {
                let name = fsutil::temporary_of(&name).unwrap_or(&name);
                kept.insert(name.to_string_lossy().into_owned(), id.to_owned());
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

/// Every distributable skill that is not `enabled = false`, in name order,
/// then `skiman-manual`, then the linked skills of the folders `offers`
/// accepts that no other skill holds the name of ([`Linked::names`]; `kept`
/// are the names links of unreadable folders keep). Folders under
/// `skills/` that are not distributable and linked skills that lose their
/// name are added to `report.items`.
pub fn wanted_entries(
    repo: &Canon,
    manifest: &Manifest,
    linked: &Linked,
    kept: &BTreeMap<String, String>,
    offers: &dyn Fn(&str) -> bool,
    report: &mut Report,
) -> std::io::Result<Vec<Wanted>> {
    let entries = manifest.skills();
    let scope = repo.skills_dir().display().to_string();
    let layout = Layout::read(&WorkingTree::new(repo.root()))?;
    let mut wanted = Vec::new();
    for placed in layout.skills() {
        let (name, dir) = (&placed.name, repo.root().join(placed.path()));
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let reason = match layout.all_of(name).len() {
            1 => not_distributable(name, &dir),
            _ => Some("its name is in more than one group folder"),
        };
        match reason {
            Some(reason) => report.items.push(Item {
                scope: scope.clone(),
                path: dir,
                kind: ItemKind::InvalidSkillFolder { reason },
            }),
            None if entries.get(name).is_some_and(|e| !e.enabled) => {}
            None => wanted.push(Wanted {
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

/// Why the skill folder `dir` (`skills/<name>` or `skills/<group>/<name>`) is not distributable; a
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

/// A tool directory of `tool`: `dir` as spelled, `real` its realpath.
struct ToolDir<'a> {
    tool: &'a AgentTool,
    dir: &'a Path,
    real: &'a Path,
}

/// Links every wanted entry in the tool directory and removes the owned
/// entries nothing wants; owned links to a `frozen` target (or below it)
/// stay exactly as they are and keep their name.
fn reconcile_tool_dir(
    tool_dir: &ToolDir,
    repo: &Canon,
    linked: &Linked,
    wanted: &[Wanted],
    frozen: &[PathBuf],
    ownership: &Ownership,
    report: &mut Report,
) {
    let ToolDir { tool, dir, real } = *tool_dir;
    let by_name: BTreeMap<&str, &Path> = wanted
        .iter()
        .map(|w| (w.name.as_str(), w.target.as_path()))
        .collect();
    let scope = real.display().to_string();
    let occupied = |name: &str, path: PathBuf| Item {
        scope: scope.clone(),
        kind: occupied(name, &path, by_name[name]),
        path,
    };
    let mut handled = BTreeSet::new();
    for (name, meta) in read_entries(dir, report) {
        let path = dir.join(&name);
        let want = name.to_str().and_then(|n| by_name.get_key_value(n));
        let entry = ownership.classify(real, &name, &meta);
        if let Entry::Owned(current) = &entry
            && frozen.iter().any(|f| current.starts_with(f))
        {
            if let Some((name, _)) = want {
                handled.insert(*name);
            }
            continue;
        }
        match (entry, want) {
            (Entry::Owned(current), Some((name, target))) => {
                handled.insert(*name);
                if current != *target {
                    match fsutil::relink(&path, real, &current, Some(target)) {
                        Ok(true) => report.changes.push((Change::Retargeted, path)),
                        Ok(false) => report.items.push(occupied(name, path)),
                        Err(e) => report
                            .errors
                            .push(format!("cannot retarget {}: {e}", path.display())),
                    }
                }
            }
            (Entry::Owned(current), None) => remove(&path, real, &current, path.clone(), report),
            (Entry::Foreign, None) => report.items.push(Item {
                scope: scope.clone(),
                kind: foreign(&name, tool.id, &path, &meta, repo, linked),
                path,
            }),
            (_, Some((name, _))) => {
                handled.insert(*name);
                report.items.push(occupied(name, path));
            }
            (Entry::ManagedElsewhere | Entry::NotASkill | Entry::Moved(_), None) => {}
        }
    }
    for Wanted { name, target } in wanted {
        if handled.contains(name.as_str()) {
            continue;
        }
        let path = dir.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                test_hook::at("sync", &format!("link:{}", path.display()));
                // `symlink` never replaces: an entry that appeared since the
                // check is occupied like one the check found.
                match std::os::unix::fs::symlink(target, &path) {
                    Ok(()) => report.changes.push((Change::Linked, path)),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        report.items.push(occupied(name, path))
                    }
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
        if let (true, Entry::Owned(current)) =
            (meta.is_symlink(), ownership.classify(real, &name, &meta))
        {
            let path = dir.join(name);
            remove(&path, real, &current, path.clone(), report);
        }
    }
}

/// Removes skiman's own link at `link` (in `real_dir`, pointing at
/// `current`), reported as `path`; one that became something else stays.
fn remove(link: &Path, real_dir: &Path, current: &Path, path: PathBuf, report: &mut Report) {
    match fsutil::relink(link, real_dir, current, None) {
        Ok(true) => report.changes.push((Change::Removed, path)),
        Ok(false) => {}
        Err(e) => report
            .errors
            .push(format!("cannot remove {}: {e}", path.display())),
    }
}
