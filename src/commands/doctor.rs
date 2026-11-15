//! `skiman doctor`: every problem of the setup in one place (stories 59, 79).
//! One line per fact on stdout: the skill repo, the recorded binary, git,
//! the git hooks and session-start hooks, each agent tool's locations and
//! aliasing, then every entry skiman does not own (foreign entries on every
//! run, occupied paths, dangling links, managed-elsewhere content), refused
//! instruction locations and override files, unresolved conflicts, leftover
//! records and tmp leftovers.
//!
//! Read-only: never writes to the skill repo, its `.skiman/` folder, tool
//! directories or machine state, and never takes the repo lock for longer
//! than a test of whether it is held. Exit 0 whenever the diagnosis ran.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup;
use crate::config::UserConfig;
use crate::failure::{Failure, finish};
use crate::git;
use crate::git_hooks::{self, Hook};
use crate::layout::Layout;
use crate::manifest::Manifest;
use crate::ownership::{Entry, Ownership};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope, rule};
use crate::session_hooks;
use crate::sync::{self, Item, ItemKind};
use crate::tools::{AgentTool, ManagedElsewhere, TOOLS};
use crate::tree::{Kind, Tree, WorkingTree};

pub fn run() -> Exit {
    finish(doctor())
}

fn doctor() -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let home = paths.home();
    let config = UserConfig::load(&paths)?;
    let repo = SkillRepo::at(&config.repo)?;
    let manifest = Manifest::load(&repo)?;
    let root = repo.root().display().to_string();
    let io = |e| Failure::operational(format!("cannot read {root}: {e}"));

    out!("skill repo: {root}");
    if config.binary.exists() {
        out!("recorded binary: {}", config.binary.display());
    } else {
        out!(
            "recorded binary: {} no longer exists; the git hooks and session-start hooks call it, so run `skiman init {root}` again with the skiman you use now",
            config.binary.display()
        );
    }
    report_git();
    report_git_hooks(&repo, &config.binary);
    let installed: Vec<&AgentTool> = sync::installed_tools(home, &manifest).collect();
    report_session_hooks(home, &installed, &root, &config.binary);
    report_tools(home, &manifest, &installed);
    report_entries(&paths, &repo, &manifest, &installed).map_err(io)?;
    report_instruction_locations(&paths, &repo, &manifest, &installed);
    report_skill_repo(&repo).map_err(io)?;
    report_tmp(&repo).map_err(io)?;
    report_projects(&paths, &repo, &manifest);
    Ok(Exit::Success)
}

fn report_git() {
    let (major, minor) = git::MIN_VERSION;
    match git::installed_version() {
        Err(_) => out!("git: not found; skiman requires git {major}.{minor} or newer"),
        Ok((version, found)) if found < git::MIN_VERSION => {
            out!("git: version {version} is too old; skiman requires {major}.{minor} or newer")
        }
        Ok((version, _)) => out!("git: version {version}"),
    }
}

fn report_git_hooks(repo: &SkillRepo, binary: &Path) {
    let root = repo.root().display();
    match git_hooks::status(repo.root(), binary) {
        Err(failure) => out!("git hooks: unknown: {}", failure.message),
        Ok(git_hooks::Status::NotAGitRepo) => {
            out!("git hooks: {root} is not in a git repo, so it has no git hooks")
        }
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
                        out!("git hook {path}: missing; run `skiman init {root}` again")
                    }
                    Hook::Outdated => out!(
                        "git hook {path}: skiman's, but not what `skiman init` writes now; run `skiman init {root}` again"
                    ),
                    Hook::Foreign => out!(
                        "git hook {path}: not skiman's; add this line to it by hand: {}",
                        git_hooks::line_to_add(&name, binary)
                    ),
                    Hook::Unreadable(e) => out!("git hook {path}: cannot read it: {e}"),
                }
            }
        }
    }
}

fn report_session_hooks(home: &Path, installed: &[&AgentTool], root: &str, binary: &Path) {
    for tool in installed.iter().filter(|t| t.session_hook_file.is_some()) {
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
            (session_hooks::Status::Different, _) => out!(
                "{prefix} {path} holds a skiman hook that differs from what `skiman init` writes now; rewrite it with `skiman init {root} --session-hooks {id}`"
            ),
            (session_hooks::Status::Unusable(reason), _) => out!("{prefix} unknown: {reason}"),
        }
    }
}

/// Each known tool: installed with its locations (and which other tool's
/// they alias), enabled without a presence directory, or not enabled.
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
        let dir_note = match dir.canonicalize() {
            Err(_) => " (missing; the next sync creates it)".to_owned(),
            Ok(real) => alias_note(&mut dirs, real, tool.id, "directory"),
        };
        let file = home.join(tool.instruction_file);
        let file_note = match sync::instruction_location(&file) {
            Err(e) => format!(" (cannot resolve its folder: {e})"),
            Ok(location) => alias_note(&mut locations, location, tool.id, "location"),
        };
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

/// Every tool-directory entry skiman does not own, classified like sync
/// does, plus the managed-elsewhere paths that exist.
fn report_entries(
    paths: &Paths,
    repo: &SkillRepo,
    manifest: &Manifest,
    installed: &[&AgentTool],
) -> std::io::Result<()> {
    let mut ignored = sync::Report::default();
    let wanted: BTreeSet<String> = sync::wanted_entries(repo, manifest, &mut ignored)?
        .into_iter()
        .map(|w| w.name)
        .collect();
    let home = paths.home();
    let ownership = Ownership::new(paths, repo, manifest, None);
    let mut managed = BTreeSet::new();
    let mut reconciled = Vec::new();
    for tool in installed {
        let dir = tool.tool_dir(home);
        let Ok(real) = dir.canonicalize() else {
            continue;
        };
        if reconciled.contains(&real) {
            continue;
        }
        let scope = real.display().to_string();
        let item = |path: PathBuf, kind| Item {
            scope: scope.clone(),
            path,
            kind,
        };
        let mut handled = BTreeSet::new();
        for (name, meta) in entries(&dir)? {
            let path = dir.join(&name);
            let want = name.to_str().filter(|n| wanted.contains(*n));
            if let Some(n) = want {
                handled.insert(n.to_owned());
            }
            match (ownership.classify(&real, &name, &meta), want) {
                (Entry::Owned(_), _) => {}
                (Entry::Moved(copy), _) => out!("{}", backup::cut_short(&path, &copy)),
                (_, Some(n)) => {
                    let kind = ItemKind::Occupied { name: n.to_owned() };
                    out!("{}", item(path, kind).message());
                }
                (Entry::Foreign, None) => {
                    let kind = ItemKind::Foreign {
                        name: name.to_string_lossy().into_owned(),
                        tool: tool.id,
                    };
                    out!("{}", item(path, kind).message());
                }
                (Entry::ManagedElsewhere, None) => {
                    managed.insert(path);
                }
                (Entry::NotASkill, None) if meta.is_symlink() && !path.exists() => out!(
                    "dangling link {} -> {}: skiman does not own it and leaves it alone; delete it if nothing needs it",
                    path.display(),
                    std::fs::read_link(&path)?.display()
                ),
                (Entry::NotASkill, None) => {}
            }
        }
        // Another spelling of a wanted name (APFS is case-insensitive) holds it.
        for name in wanted.difference(&handled) {
            let path = dir.join(name);
            if std::fs::symlink_metadata(&path).is_ok() {
                let kind = ItemKind::Occupied { name: name.clone() };
                out!("{}", item(path, kind).message());
            }
        }
        reconciled.push(real);
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
    Ok(())
}

/// The entries of `dir` with their own metadata, sorted by name; none when `dir` is missing.
fn entries(dir: &Path) -> std::io::Result<Vec<(OsString, std::fs::Metadata)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        out.push((entry.file_name(), std::fs::symlink_metadata(entry.path())?));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Instruction locations skiman does not own, and files a tool loads instead of them.
fn report_instruction_locations(
    paths: &Paths,
    repo: &SkillRepo,
    manifest: &Manifest,
    installed: &[&AgentTool],
) {
    let home = paths.home();
    let ownership = Ownership::new(paths, repo, manifest, None);
    let mut seen = Vec::new();
    for tool in installed {
        let path = home.join(tool.instruction_file);
        match sync::instruction_location(&path) {
            Err(e) => out!(
                "instruction file {}: cannot resolve its folder: {e}",
                path.display()
            ),
            Ok(location) if seen.contains(&location) => {}
            Ok(location) => {
                if let Ok(meta) = std::fs::symlink_metadata(&location)
                    && ownership.owned_instruction(&location, &meta).is_none()
                {
                    let item = Item {
                        scope: location.display().to_string(),
                        path: path.clone(),
                        kind: ItemKind::RefusedLocation,
                    };
                    out!("{}", item.message());
                }
                seen.push(location);
            }
        }
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
    }
}

/// Unresolved conflicts in every skill folder, and leftover records.
fn report_skill_repo(repo: &SkillRepo) -> std::io::Result<()> {
    let tree = WorkingTree::new(repo.root());
    for placed in Layout::read(&tree)?.skills() {
        if tree.kind(&placed.path())? == Some(Kind::Dir) {
            for violation in rules::unresolved_conflicts(&tree, &placed.name)? {
                out!("{violation}");
            }
        }
    }
    let scope = Scope {
        consistency: Scope::all(&tree)?.consistency,
        ..Scope::default()
    };
    for violation in rules::check(&tree, &scope)? {
        if violation.rule == rule::LEFTOVER_RECORD {
            out!("{violation}");
        }
    }
    Ok(())
}

/// Every entry of `.skiman/tmp/`: a tmp leftover, or in use by the command
/// that holds the repo lock.
fn report_tmp(repo: &SkillRepo) -> std::io::Result<()> {
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
    let in_use = RepoLock::is_held(repo)?;
    for path in leftovers {
        if in_use {
            out!(
                "{} is in use: another skiman command holds the repo lock",
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

/// The project scope: every root, every item that is not in step with
/// its fix, managed items a project repo now tracks, candidates not adopted
/// yet, items only the mirror holds, staging folders left by an earlier
/// write, mirror paths git ignores, and uncommitted mirror changes.
fn report_projects(paths: &Paths, repo: &SkillRepo, manifest: &Manifest) {
    use crate::project::state::{State, classify};
    use crate::project::{command, discover, git as pgit, item, root, synced::Synced};
    if manifest.projects().is_empty() {
        return;
    }
    if let Some(operation) = pgit::operation_in_progress(repo.root()) {
        out!("project items: not synced while the skill repo {operation}");
    }
    let synced = Synced::load(paths);
    let tree = WorkingTree::new(repo.root());
    let skip = discover::never_entered(paths, repo);
    let mut mirror_paths = Vec::new();
    for resolved in root::resolve(paths.home(), repo, manifest) {
        let root = match resolved {
            Ok(root) => root,
            Err(problem) => {
                out!("project root {}: skipped: {}", problem.id, problem.message);
                continue;
            }
        };
        let items = match item::managed(&tree, &root, &synced) {
            Ok(items) => items,
            Err(e) => {
                out!("project root {}: cannot read its mirror: {e}", root.id);
                continue;
            }
        };
        let in_mirror = item::read_mirror(&tree, &root)
            .map(|m| m.items)
            .unwrap_or_default();
        mirror_paths.extend(in_mirror.iter().map(item::Item::mirror_rel));
        out!(
            "project root {}: {} ({} item(s) in the mirror)",
            root.id,
            root.dir.display(),
            in_mirror.len()
        );
        let (mut not_here, mut on_disk) = (0, Vec::new());
        for item in &items {
            let recorded = synced.get(&root, &item.rel).map(|(_, h)| h.clone());
            let Ok(evaluated) = classify(repo, &root, item, recorded.as_deref()) else {
                out!("{}: cannot read it", item.disk(&root).display());
                continue;
            };
            if evaluated.disk_hash.is_some() {
                on_disk.push(item.disk(&root));
            }
            match evaluated.state {
                State::InSync | State::ProjectChanged => {}
                State::NotHere => not_here += 1,
                State::Forgotten | State::Nothing | State::New => {}
                _ => out!("{}", evaluated.message(&root, item)),
            }
        }
        if not_here > 0 {
            out!(
                "project root {}: {not_here} item(s) are only in the mirror: their project folders are not on this machine",
                root.id
            );
        }
        for (disk, tracking) in pgit::tracking_repos(&on_disk) {
            if let Ok(Some(top)) = tracking {
                out!(
                    "{}: now tracked by the git repo {}; leave it to that repo with: {}",
                    disk.display(),
                    top.display(),
                    command("eject", std::slice::from_ref(&disk))
                );
            }
        }
        let found = discover::discover(&root, &skip);
        let candidates: Vec<std::path::PathBuf> = found
            .items
            .iter()
            .filter(|i| !items.iter().any(|m| m.rel == i.rel))
            .map(|i| i.disk(&root))
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
        out!("{path}: ignored by git ({rule}), so it never gets history; change that rule");
    }
    match pgit::uncommitted_mirror_changes(repo.root()) {
        Some(0) | None => {}
        Some(n) => out!(
            "{}: {n} uncommitted change(s); skiman never commits, so commit them to keep their history",
            item::MIRROR_DIR
        ),
    }
}
