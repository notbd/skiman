//! `skiman doctor`: every problem of the setup in one place (stories 59, 79).
//! One line per fact on stdout: the canon, the recorded binary, git,
//! the git hooks and session-start hooks, each agent tool's locations and
//! aliasing; then what the next sync does and reports, from sync's own
//! decisions (`sync::Output::Plan`): each link it makes, retargets or
//! removes and each project change it mirrors (`<path>: the next \`skiman
//! sync\` links it`), each past the read-only checks sync makes before it
//! writes, and everything it reports (foreign entries every time); then
//! dangling links, managed-elsewhere content, override and added
//! instruction files, unresolved conflicts, leftover records, tmp leftovers
//! and the project scope. Not foreseen: what only the write shows
//! (permissions, a full disk, a change in between), nor what a link made
//! by hand into what skiman manages leads to once sync changed it.
//!
//! Writes no file, and never takes the canon lock for longer than a test of
//! whether it is held; its git queries take no optional lock, so the index
//! stays as it is (git still runs the user's clean and process filters).
//! Exit 3 when a line needs the user (what would make `skiman sync` exit
//! 3, and each line [`problem!`] prints), 4 when something it checks
//! cannot be read or used, otherwise 0.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup;
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::config::UserConfig;
use crate::failure::{Failure, finish};
use crate::git;
use crate::git_hooks::{self, Hook};
use crate::layout::Layout;
use crate::linked::{Linked, State};
use crate::manifest::Manifest;
use crate::ownership::Entry;
use crate::paths::Paths;
use crate::project::{item, root};
use crate::rules::{self, Scope, rule};
use crate::session_hooks;
use crate::sync::{self, Change, Output};
use crate::tools::{AgentTool, ManagedElsewhere, TOOLS};
use crate::tree::{Kind, Tree, WorkingTree};

pub fn run() -> Exit {
    finish(doctor())
}

/// `out!` for a line that needs the user: `exit` (an `&mut Exit`) becomes 3.
macro_rules! problem {
    ($exit:expr, $($arg:tt)*) => {{
        *$exit = $exit.combine(Exit::NeedsAttention);
        out!($($arg)*)
    }};
}

fn doctor() -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let home = paths.home();
    let config = UserConfig::load(&paths)?;
    let repo = Canon::at(&config.repo)?;
    let manifest = Manifest::load(&repo)?;
    let root = repo.root().display().to_string();
    let io = |e| Failure::operational(format!("cannot read {root}: {e}"));

    let exit = &mut Exit::Success;
    out!("canon: {root}");
    if config.binary.exists() {
        out!("recorded binary: {}", config.binary.display());
    } else {
        problem!(
            exit,
            "recorded binary: {} no longer exists; the git hooks and session-start hooks call it, so run `skiman init {root}` again with the skiman you use now",
            config.binary.display()
        );
    }
    report_git(exit);
    report_git_hooks(&repo, &config.binary, exit);
    let installed: Vec<&AgentTool> = sync::installed_tools(home, &manifest).collect();
    report_session_hooks(home, &installed, (&root, &config.binary), exit);
    let linked = Linked::read(home, &repo, &manifest);
    let plan = sync::reconcile(&paths, &repo, &manifest, &linked, None, Output::Plan);
    report_tools(home, &manifest, &installed);
    report_distribution(home, (&linked, &plan), exit);
    report_instruction_files(home, &installed);
    report_canon(&repo, exit).map_err(io)?;
    report_tmp(&repo).map_err(io)?;
    report_projects(&paths, (&repo, &manifest, &linked), exit);
    Ok(*exit)
}

/// Prints what a sync plan would do and report; `exit` takes the exit the
/// sync would have.
fn report_plan(report: &sync::Report, exit: &mut Exit) {
    for (change, path) in &report.changes {
        let verb = match change {
            Change::Linked => "links",
            Change::Retargeted => "retargets",
            Change::Removed => "removes",
            Change::Mirrored => "mirrors",
        };
        out!("{}: the next `skiman sync` {verb} it", path.display());
    }
    for item in &report.items {
        out!("{}", item.message());
    }
    for error in &report.errors {
        out!("{error}");
    }
    *exit = exit.combine(report.exit());
}

fn report_git(exit: &mut Exit) {
    let (major, minor) = git::MIN_VERSION;
    match git::installed_version() {
        Err(_) => problem!(
            exit,
            "git: not found; skiman requires git {major}.{minor} or newer"
        ),
        Ok((version, found)) if found < git::MIN_VERSION => {
            problem!(
                exit,
                "git: version {version} is too old; skiman requires {major}.{minor} or newer"
            )
        }
        Ok((version, _)) => out!("git: version {version}"),
    }
}

fn report_git_hooks(repo: &Canon, binary: &Path, exit: &mut Exit) {
    let root = repo.root().display();
    match git_hooks::status(repo.root(), binary) {
        Err(failure) => problem!(exit, "git hooks: unknown: {}", failure.message),
        Ok(git_hooks::Status::NotAGitRepo) => {
            out!("git hooks: {root} is not in a git repo, so it has no git hooks")
        }
        // Not a problem: doctor cannot tell whether the hooks there hold the lines.
        Ok(git_hooks::Status::HooksPathElsewhere(dir)) => out!(
            "git hooks: core.hooksPath points to {}, so skiman's git hooks do not run; add skiman's lines to the hooks there by hand",
            dir.display()
        ),
        Ok(git_hooks::Status::Hooks { dir, hooks }) => {
            if hooks.iter().all(|(_, hook)| *hook == Hook::Installed) {
                out!("git hooks: installed in {}", dir.display());
            }
            for (path, hook) in hooks {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let path = path.display();
                match hook {
                    Hook::Installed => {}
                    Hook::Missing => {
                        problem!(
                            exit,
                            "git hook {path}: missing; run `skiman init {root}` again"
                        )
                    }
                    Hook::Outdated => problem!(
                        exit,
                        "git hook {path}: skiman's, but not what `skiman init` writes now; run `skiman init {root}` again"
                    ),
                    // Not a problem: doctor cannot tell whether it holds the line.
                    Hook::Foreign => out!(
                        "git hook {path}: not skiman's; add this line to it by hand: {}",
                        git_hooks::line_to_add(&name, binary)
                    ),
                    Hook::Unreadable(e) => problem!(exit, "git hook {path}: cannot read it: {e}"),
                }
            }
        }
    }
}

fn report_session_hooks(
    home: &Path,
    installed: &[&AgentTool],
    (root, binary): (&str, &Path),
    exit: &mut Exit,
) {
    for tool in installed.iter().filter(|t| t.session_hook.is_some()) {
        let id = tool.id;
        let (path, status) = session_hooks::status(home, tool, binary);
        let path = path.display();
        let prefix = format!("session-start hook for {id}:");
        match (status, tool.session_hook_unavailable) {
            (session_hooks::Status::Installed, _) => out!("{prefix} installed in {path}"),
            (session_hooks::Status::Missing, Some(reason)) => {
                out!("{prefix} not added: {reason}")
            }
            (session_hooks::Status::Missing, None) => out!(
                "{prefix} not installed in {path}; add it with `skiman init {root} --session-hooks {id}`"
            ),
            (session_hooks::Status::Different, _) => problem!(
                exit,
                "{prefix} {path} holds a skiman hook that differs from what `skiman init` writes now; rewrite it with `skiman init {root} --session-hooks {id}`"
            ),
            (session_hooks::Status::Unusable(reason), _) => {
                problem!(exit, "{prefix} unknown: {reason}")
            }
        }
    }
}

/// Each known tool: installed with its locations (and which other tool's
/// they alias), enabled without a presence directory, or not enabled.
/// Whether sync can use or make them is said by its plan, not here.
fn report_tools(home: &Path, manifest: &Manifest, installed: &[&AgentTool]) {
    let enabled = manifest.enabled_tools();
    let mut dirs: Vec<(PathBuf, &str)> = Vec::new();
    let mut locations: Vec<(PathBuf, &str)> = Vec::new();
    for tool in TOOLS {
        if !enabled.iter().any(|id| id == tool.id) {
            out!("{}: not enabled", tool.id);
            continue;
        }
        if !installed.contains(&tool) {
            out!(
                "{}: enabled, but {} does not exist, so skiman skips it",
                tool.id,
                tool.presence_dir(home).display()
            );
            continue;
        }
        let dir = tool.tool_dir(home);
        let dir_note = dir
            .canonicalize()
            .ok()
            .map(|real| alias_note(&mut dirs, real, tool.id, "directory"))
            .unwrap_or_default();
        let file = home.join(tool.instruction_file);
        let file_note = sync::instruction_location(&file)
            .ok()
            .map(|at| alias_note(&mut locations, at, tool.id, "location"))
            .unwrap_or_default();
        out!(
            "{}: installed; tool directory {}{dir_note}; instruction file {}{file_note}",
            tool.id,
            dir.display(),
            file.display()
        );
    }
}

/// `" (same <what> as <id>'s)"` when `real` was seen for another tool; records it otherwise.
fn alias_note<'a>(
    seen: &mut Vec<(PathBuf, &'a str)>,
    real: PathBuf,
    id: &'a str,
    what: &str,
) -> String {
    match seen.iter().find(|(path, _)| *path == real) {
        Some((_, other)) => format!(" (same {what} as {other}'s)"),
        None => {
            seen.push((real, id));
            String::new()
        }
    }
}

/// Each linked folder, then what the next `skiman sync` does and reports,
/// decided by sync's own reconcile without writing ([`sync::Output::Plan`]):
/// the links it would make, retarget or remove, everything it would report
/// (foreign entries every time), then what it leaves alone without a word:
/// moves cut short, dangling links and managed-elsewhere content. `exit`
/// takes the exit that sync would have.
fn report_distribution(home: &Path, (linked, report): (&Linked, &sync::Report), exit: &mut Exit) {
    for folder in &linked.folders {
        if let (State::Read(_), Some(path)) = (&folder.state, &folder.path) {
            let names = Linked::skill_names(folder);
            let skills = if Linked::is_single(folder) {
                "one skill".to_owned()
            } else if names.is_empty() {
                "no skills yet".to_owned()
            } else {
                format!("skills: {}", names.join(", "))
            };
            out!("linked folder {}: {} ({skills})", folder.id, path.display());
        }
    }
    report_plan(report, exit);
    let mut managed = BTreeSet::new();
    for (path, entry) in &report.left {
        match entry {
            Entry::Moved(copy) => out!("{}", backup::cut_short(path, copy)),
            Entry::ManagedElsewhere => {
                managed.insert(path.clone());
            }
            Entry::NotASkill if path.is_symlink() && !path.exists() => out!(
                "dangling link {} -> {}: skiman does not own it and leaves it alone; delete it if nothing needs it",
                path.display(),
                std::fs::read_link(path).unwrap_or_default().display()
            ),
            _ => {}
        }
    }
    for tool in TOOLS {
        for entry in tool.managed_elsewhere {
            if let ManagedElsewhere::Path(rel) = entry {
                let path = home.join(rel);
                if std::fs::symlink_metadata(&path).is_ok() {
                    managed.insert(path);
                }
            }
        }
    }
    for path in managed {
        out!(
            "managed elsewhere: {}; skiman never touches it",
            path.display()
        );
    }
}

/// Files a tool loads instead of, or on top of, its instruction location.
fn report_instruction_files(home: &Path, installed: &[&AgentTool]) {
    for tool in installed {
        let path = home.join(tool.instruction_file);
        for rel in tool.instruction_overrides {
            let file = home.join(rel);
            if std::fs::symlink_metadata(&file).is_ok() {
                out!(
                    "{} exists, so {} loads it instead of {}; move it away to use the shared body",
                    file.display(),
                    tool.id,
                    path.display()
                );
            }
        }
        for rel in tool.instruction_additions {
            let file = home.join(rel);
            if std::fs::symlink_metadata(&file).is_ok() {
                out!(
                    "{} exists, so {} also loads it on top of {}",
                    file.display(),
                    tool.id,
                    path.display()
                );
            }
        }
    }
}

/// Unresolved conflicts in every skill folder, and leftover records.
fn report_canon(repo: &Canon, exit: &mut Exit) -> std::io::Result<()> {
    let tree = WorkingTree::new(repo.root());
    for placed in Layout::read(&tree)?.skills() {
        if tree.kind(&placed.path())? == Some(Kind::Dir) {
            for violation in rules::unresolved_conflicts(&tree, &placed.name)? {
                problem!(exit, "{violation}");
            }
        }
    }
    let scope = Scope {
        consistency: Scope::all(&tree)?.consistency,
        ..Scope::default()
    };
    for violation in rules::check(&tree, &scope)? {
        if violation.rule == rule::LEFTOVER_RECORD {
            problem!(exit, "{violation}");
        }
    }
    Ok(())
}

/// Every entry of `.skiman/tmp/`: a tmp leftover, or in use by the command
/// that holds the canon lock.
fn report_tmp(repo: &Canon) -> std::io::Result<()> {
    let mut leftovers = match std::fs::read_dir(repo.tmp_dir()) {
        Ok(read) => read
            .map(|entry| entry.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if leftovers.is_empty() {
        return Ok(());
    }
    leftovers.sort();
    let in_use = CanonLock::is_held(repo)?;
    for path in leftovers {
        if in_use {
            out!(
                "{} is in use: another skiman command holds the canon lock",
                path.display()
            );
        } else {
            out!(
                "tmp leftover {}; the next write command moves it to a backup",
                path.display()
            );
        }
    }
    Ok(())
}

/// The project scope: every root, and why one cannot be used (not on this
/// machine 0, a wrong configuration 3, a path that cannot be read 4), also
/// while project sync is paused; then what the next sync reports about the
/// items, judged by project sync itself on the same roots (also while a git
/// operation in the canon pauses it); then managed items a project repo
/// now tracks, candidates not adopted yet and those skipped as overlapping
/// a linked folder, staging folders left by an earlier write, mirror paths
/// git ignores, and uncommitted mirror changes.
fn report_projects(
    paths: &Paths,
    (repo, manifest, linked): (&Canon, &Manifest, &Linked),
    exit: &mut Exit,
) {
    use crate::project::{command, discover, git as pgit, synced::Synced};
    if manifest.projects().is_empty() {
        return;
    }
    let synced = Synced::load(paths);
    let tree = WorkingTree::new(repo.root());
    let resolved = root::resolve(paths.home(), repo, manifest);
    let mut mirror_paths = Vec::new();
    for root in &resolved {
        let root = match root {
            Ok(root) => root,
            Err(problem) => {
                let severity = match problem.kind {
                    root::Kind::Absent => Exit::Success,
                    root::Kind::Invalid => Exit::NeedsAttention,
                    root::Kind::Unreadable => Exit::OperationalError,
                };
                *exit = exit.combine(severity);
                out!("{}", problem.skipped());
                continue;
            }
        };
        let in_mirror = item::read_mirror(&tree, root)
            .map(|m| m.items)
            .unwrap_or_default();
        mirror_paths.extend(in_mirror.iter().map(item::Item::mirror_rel));
        out!(
            "project root {}: {} ({} item(s) in the mirror)",
            root.id,
            root.dir.display(),
            in_mirror.len()
        );
    }
    let mut report = sync::Report::default();
    let records = (manifest, linked, Some(resolved.as_slice()));
    crate::project::sync::reconcile(paths, repo, records, Output::Plan, &mut report);
    report_plan(&report, exit);
    // `project adopt` refuses then, so doctor does not point to it.
    let paused = crate::project::paused_by(repo, linked);
    if let Some(why) = &paused {
        out!("project items not adopted yet are not counted: {why}");
    }
    let skip = discover::never_entered(paths, repo);
    for root in resolved.iter().flatten() {
        let Ok(items) = item::managed(&tree, root, &synced) else {
            continue;
        };
        // Sync's plan names a managed item that overlaps a linked folder.
        let on_disk: Vec<_> = items
            .iter()
            .map(|i| i.disk(root))
            .filter(|d| linked.item_overlap(d).is_none() && std::fs::symlink_metadata(d).is_ok())
            .collect();
        for (disk, tracking) in pgit::tracking_repos(&on_disk) {
            if let Ok(Some(top)) = tracking {
                problem!(
                    exit,
                    "{}: now tracked by the git repo {}; leave it to that repo with: {}",
                    disk.display(),
                    top.display(),
                    command("eject", std::slice::from_ref(&disk))
                );
            }
        }
        let found = discover::discover(root, &skip, linked);
        for error in &found.errors {
            out!("{error}");
            *exit = exit.combine(Exit::OperationalError);
        }
        let adopted = |rel: &str| items.iter().any(|m| m.rel == rel);
        for (item, why) in found.skipped.iter().filter(|(i, _)| !adopted(&i.rel)) {
            out!(
                "project root {}: {} is skipped: {why}",
                root.id,
                item.disk(root).display()
            );
        }
        let candidates: Vec<PathBuf> = found
            .items
            .iter()
            .filter(|i| paused.is_none() && !adopted(&i.rel))
            .map(|i| i.disk(root))
            .collect();
        let new = pgit::tracking_repos(&candidates)
            .values()
            .filter(|t| matches!(t, Ok(None)))
            .count();
        if new > 0 {
            out!(
                "project root {}: {new} project item(s) are not adopted yet; see them with: {} --dry-run",
                root.id,
                command("adopt", std::slice::from_ref(&root.dir))
            );
        }
        for leftover in found.swap_leftovers {
            out!(
                "{} is left from an earlier `skiman project restore`; the next restore there moves it to a backup",
                leftover.display()
            );
        }
    }
    let (swap_dir, _) = crate::project::sync::swap_parent(repo);
    for leftover in crate::staging::list(&swap_dir).unwrap_or_default() {
        out!(
            "{} is left from an earlier mirror write; the next mirror write moves it to a backup",
            leftover.display()
        );
    }
    for (path, rule) in pgit::ignored(repo.root(), &mirror_paths) {
        problem!(
            exit,
            "{path}: ignored by git ({rule}), so it never gets history; change that rule"
        );
    }
    match pgit::uncommitted_mirror_changes(repo.root()) {
        Some(0) | None => {}
        Some(n) => out!(
            "{}: {n} uncommitted change(s); skiman never commits, so commit them to keep their history",
            item::MIRROR_DIR
        ),
    }
}
