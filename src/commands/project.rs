//! `skiman project add|adopt|restore|eject` (plan "Commands"). One rule for
//! all of them: a command given a folder never replaces existing content or
//! undoes a local deletion on either side; that needs the item's exact path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup::Backup;
use crate::commands::{confirm, has_terminal};
use crate::failure::{Failure, finish};
use crate::fsutil::{absolute_lexical, write_atomic};
use crate::manifest::{self, Manifest};
use crate::paths::Paths;
use crate::project::content::{self, Side};
use crate::project::discover::discover;
use crate::project::git::{ignored, operation_in_progress, tracking_repos};
use crate::project::item::{Item, MIRROR_DIR, read_mirror, read_mirror_of};
use crate::project::root::{self, Root};
use crate::project::state::{Evaluated, State, evaluate};
use crate::project::swap::{self, Backups, Expect, Outcome, Place};
use crate::project::sync::{check_rules, swap_parent};
use crate::project::synced::Synced;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::sync::{self, Output};
use crate::tree::{Overlay, WorkingTree};

// ---------------------------------------------------------------- add

pub fn add(path: &Path, id: Option<String>) -> Exit {
    finish(run_add(path, id))
}

fn run_add(path: &Path, id: Option<String>) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let home = paths.home();
    let dir = cwd_absolute(path)?
        .canonicalize()
        .ok()
        .filter(|d| d.is_dir())
        .ok_or_else(|| Failure::refused(format!("{} is not an existing folder", path.display())))?;
    root::validate_dir(home, &repo, &dir).map_err(Failure::refused)?;
    let id = match id {
        Some(id) => id,
        None => root::default_id(&dir),
    };
    if !crate::names::is_valid(&id) {
        return Err(Failure::refused(format!(
            "`{id}` is not a valid project root id (lowercase letters, digits and single hyphens); give one with --id"
        )));
    }
    let _lock = RepoLock::wait(&repo)?;
    let manifest = Manifest::load(&repo)?;
    for entry in manifest.projects() {
        if entry.id == id {
            return Err(Failure::refused(format!(
                "skiman.toml already has [projects.{id}]; pick another id with --id"
            )));
        }
        let other = entry.path.as_deref().and_then(|p| root::expand(home, p));
        let other = other.map(|p| p.canonicalize().unwrap_or(p));
        if let Some(other) = other
            && (other.starts_with(&dir) || dir.starts_with(&other))
        {
            return Err(Failure::refused(format!(
                "{} overlaps project root `{}` ({}); roots must not contain each other",
                dir.display(),
                entry.id,
                other.display()
            )));
        }
    }
    let file = repo.manifest_file();
    let text = read_text(&file)?;
    let written = root::written_form(home, &dir);
    let text = manifest::with_project(&text, &id, &written)
        .map_err(|e| Failure::refused(format!("{}: {e}", file.display())))?;
    let tree = WorkingTree::new(repo.root());
    let candidate = Overlay::new(
        &tree,
        vec![(rules::MANIFEST.to_owned(), text.clone().into_bytes())],
    );
    rules::validate_write(
        &candidate,
        &Scope {
            manifest: true,
            ..Scope::default()
        },
    )?;
    write_atomic(&file, text.as_bytes())
        .map_err(|e| Failure::operational(format!("cannot write {}: {e}", file.display())))?;
    println!("registered project root {id}: {}", dir.display());
    println!(
        "its project files will be copied into {}/{MIRROR_DIR}/{id}/ and pushed wherever the skill repo is pushed; see what it holds with `skiman project adopt {} --dry-run`",
        repo.root().display(),
        dir.display()
    );
    Ok(Exit::Success)
}

// ---------------------------------------------------------------- shared

/// What a command works on: the lock, the configuration and the usable roots.
struct Context {
    paths: Paths,
    repo: SkillRepo,
    lock: RepoLock,
    manifest: Manifest,
    roots: Vec<Root>,
}

impl Context {
    /// Takes the lock and refuses (exit 4) while the skill repo is in the
    /// middle of a git operation.
    fn open() -> Result<Context, Failure> {
        let paths = Paths::from_env()?;
        let repo = SkillRepo::from_config(&paths)?;
        let lock = RepoLock::wait(&repo)?;
        if let Some(operation) = operation_in_progress(repo.root()) {
            return Err(Failure::operational(format!(
                "the skill repo is in the middle of {operation}; finish it first, so nothing is written into an old checkout or a half-merged tree"
            )));
        }
        let manifest = Manifest::load(&repo)?;
        let mut roots = Vec::new();
        for resolved in root::resolve(paths.home(), &repo, &manifest) {
            match resolved {
                Ok(root) => roots.push(root),
                Err(problem) => eprintln!(
                    "warning: project root `{}` is skipped: {}",
                    problem.id, problem.message
                ),
            }
        }
        Ok(Context {
            paths,
            repo,
            lock,
            manifest,
            roots,
        })
    }

    /// The root a path given on the command line is in, and the path
    /// relative to it (`""` for the root itself). The path is compared by
    /// realpath, as far as it exists.
    fn locate(&self, path: &Path) -> Result<(&Root, String), Failure> {
        let real = realpath_as_far_as_exists(&cwd_absolute(path)?);
        self.roots
            .iter()
            .find_map(|root| {
                let rel = real.strip_prefix(&root.dir).ok()?;
                Some((root, rel.to_string_lossy().into_owned()))
            })
            .ok_or_else(|| {
                Failure::refused(format!(
                    "{} is not inside a project root (`skiman project add <folder>` registers one)",
                    path.display()
                ))
            })
    }
}

/// One path the user named, resolved.
struct Target<'a> {
    root: &'a Root,
    rel: String,
    /// The path names an item exactly.
    exact: Option<Item>,
}

fn targets<'a>(ctx: &'a Context, given: &[PathBuf]) -> Result<Vec<Target<'a>>, Failure> {
    if given.is_empty() {
        return Ok(ctx
            .roots
            .iter()
            .map(|root| Target {
                root,
                rel: String::new(),
                exact: None,
            })
            .collect());
    }
    given
        .iter()
        .map(|path| {
            let (root, rel) = ctx.locate(path)?;
            let is_dir = std::fs::symlink_metadata(root.dir.join(&rel)).is_ok_and(|m| m.is_dir());
            let exact = (!rel.is_empty())
                .then(|| Item::at(root, &rel, is_dir).or_else(|| Item::at(root, &rel, !is_dir)))
                .flatten();
            Ok(Target { root, rel, exact })
        })
        .collect()
}

fn under(rel: &str, folder: &str) -> bool {
    folder.is_empty() || rel == folder || rel.starts_with(&format!("{folder}/"))
}

/// Every item the mirror or the synced hashes know under `target`.
fn known_items(ctx: &Context, synced: &Synced, target: &Target) -> Result<Vec<Item>, Failure> {
    let tree = WorkingTree::new(ctx.repo.root());
    let mirror = read_mirror(&tree, target.root).map_err(|e| {
        Failure::operational(format!("cannot read {}: {e}", ctx.repo.root().display()))
    })?;
    let mut items: BTreeMap<String, Item> = mirror
        .items
        .into_iter()
        .map(|i| (i.rel.clone(), i))
        .collect();
    for (rel, kind) in synced.items(target.root) {
        if let Some(item) = Item::at(
            target.root,
            &rel,
            kind == crate::project::pattern::Kind::Skill,
        ) {
            items.entry(rel).or_insert(item);
        }
    }
    Ok(items
        .into_values()
        .filter(|i| match &target.exact {
            Some(exact) => i.rel == exact.rel,
            None => under(&i.rel, &target.rel),
        })
        .collect())
}

fn evaluated(
    ctx: &Context,
    synced: &Synced,
    root: &Root,
    item: &Item,
) -> Result<Evaluated, Failure> {
    let recorded = synced.get(root, &item.rel).map(|(_, h)| h.clone());
    evaluate(root, item, ctx.repo.root(), recorded.as_deref()).map_err(|e| {
        Failure::operational(format!("cannot read {}: {e}", item.disk(root).display()))
    })
}

/// Refuses (exit 2) a command that would ask without a terminal.
fn need_yes(yes: bool, dry_run: bool) -> Result<(), Failure> {
    if !yes && !dry_run && !has_terminal() {
        return Err(Failure::usage("missing flag without a terminal: --yes"));
    }
    Ok(())
}

/// One planned write.
struct Planned<'a> {
    root: &'a Root,
    item: Item,
    eval: Evaluated,
    /// The verb printed in the plan.
    what: &'static str,
}

/// Skipped candidates, printed after the plan.
#[derive(Default)]
struct Skips {
    lines: Vec<String>,
    /// A path the user named exactly was skipped (exit 1).
    named: bool,
}

impl Skips {
    fn add(&mut self, named: bool, line: String) {
        self.named |= named;
        self.lines.push(line);
    }
}

/// Gate 1 on `planned`: drops every item a project's own git repo tracks,
/// or whose repo git cannot read.
fn drop_tracked(planned: &mut Vec<Planned>, named: &BTreeSet<PathBuf>, skips: &mut Skips) {
    let disks: Vec<PathBuf> = planned.iter().map(|p| p.item.disk(p.root)).collect();
    let tracking = tracking_repos(&disks);
    let mut by_repo: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    planned.retain(|p| {
        let disk = p.item.disk(p.root);
        match tracking.get(&disk) {
            Some(Ok(Some(top))) => {
                by_repo.entry(top.clone()).or_default().push(disk.clone());
                skips.named |= named.contains(&disk);
                false
            }
            Some(Err(e)) => {
                skips.add(
                    named.contains(&disk),
                    format!("skip {}: {e}", disk.display()),
                );
                false
            }
            _ => true,
        }
    });
    for (top, disks) in by_repo {
        skips.lines.push(format!(
            "skip {} item(s) tracked by the git repo {} (that repo already keeps their history):",
            disks.len(),
            top.display()
        ));
        for disk in disks {
            skips.lines.push(format!("  {}", disk.display()));
        }
    }
}

/// Prints the plan and the skips; asks unless `yes`. `false` when there is
/// nothing to do or the user said no.
fn show_plan(
    planned: &[Planned],
    skips: &Skips,
    verb: &str,
    yes: bool,
    dry_run: bool,
) -> Result<bool, Failure> {
    for p in planned {
        println!("{} {}", p.what, p.item.disk(p.root).display());
    }
    for line in &skips.lines {
        eprintln!("{line}");
    }
    if planned.is_empty() {
        println!("nothing to {verb}");
        return Ok(false);
    }
    if dry_run {
        return Ok(false);
    }
    if !yes
        && !confirm(&format!(
            "{} {} item(s)? [y/N] ",
            capitalise(verb),
            planned.len()
        ))?
    {
        return Err(Failure::refused(format!(
            "not {verb}ed; nothing was changed"
        )));
    }
    Ok(true)
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// Ends a write command: saves the synced hashes, reports the backup, and
/// syncs. `own` is the exit status of the command's own work.
fn finish_write(
    ctx: &Context,
    synced: &mut Synced,
    backups: &Backups,
    own: Exit,
) -> Result<Exit, Failure> {
    synced.save(&ctx.paths).map_err(|e| {
        Failure::operational(format!(
            "cannot write {}: {e}",
            ctx.paths.state_dir().display()
        ))
    })?;
    if let Some(dir) = backups.created() {
        println!("replaced versions are in {}", dir.display());
    }
    Ok(own.combine(sync::run(&ctx.paths, &ctx.repo, &ctx.lock, Output::Normal)))
}

/// Applies one replace and prints its outcome; returns the exit it adds.
fn report_outcome(outcome: std::io::Result<Outcome>, done: &str, disk: &Path) -> Exit {
    match outcome {
        Ok(Outcome::Replaced { backup, late_write }) => {
            println!("{done} {}", disk.display());
            if late_write && let Some(backup) = backup {
                eprintln!(
                    "warning: a write landed while {} was replaced; that version is in {}",
                    disk.display(),
                    backup.display()
                );
                return Exit::NeedsAttention;
            }
            Exit::Success
        }
        Ok(Outcome::Conflict(why)) => {
            eprintln!("warning: {why}; nothing was replaced, run the command again");
            Exit::NeedsAttention
        }
        Ok(Outcome::Kept(why)) => {
            eprintln!("error: {why}");
            Exit::OperationalError
        }
        Err(e) => {
            eprintln!("error: cannot write {}: {e}", disk.display());
            Exit::OperationalError
        }
    }
}

// ---------------------------------------------------------------- adopt

pub fn adopt(paths: Vec<PathBuf>, dry_run: bool, yes: bool) -> Exit {
    finish(run_adopt(paths, dry_run, yes))
}

fn run_adopt(given: Vec<PathBuf>, dry_run: bool, yes: bool) -> Result<Exit, Failure> {
    need_yes(yes, dry_run)?;
    let ctx = Context::open()?;
    let mut synced = Synced::load(&ctx.paths);
    let targets = targets(&ctx, &given)?;
    let skip_dirs = vec![
        ctx.repo.root().to_path_buf(),
        ctx.paths
            .state_dir()
            .canonicalize()
            .unwrap_or(ctx.paths.state_dir()),
    ];
    let mut named = BTreeSet::new();
    let mut planned: Vec<Planned> = Vec::new();
    let mut skips = Skips::default();
    let mut seen = BTreeSet::new();
    for target in &targets {
        let candidates: Vec<Item> = match &target.exact {
            Some(item) => {
                named.insert(item.disk(target.root));
                vec![item.clone()]
            }
            None => {
                let found = discover(target.root, &skip_dirs);
                for e in found.errors {
                    eprintln!("warning: {e}");
                }
                for leftover in found.swap_leftovers {
                    eprintln!(
                        "warning: {} is left from an interrupted `skiman project restore`; the next restore there moves what it holds to a backup",
                        leftover.display()
                    );
                }
                let mut items = found.items;
                items.retain(|i| under(&i.rel, &target.rel));
                items
            }
        };
        if target.exact.is_none() && !target.rel.is_empty() && candidates.is_empty() {
            let path = target.root.dir.join(&target.rel);
            if !path.is_dir() {
                return Err(Failure::refused(format!(
                    "{} is neither a project item nor a folder holding one",
                    path.display()
                )));
            }
        }
        for item in candidates {
            if !seen.insert((item.root.clone(), item.rel.clone())) {
                continue;
            }
            let exact = target.exact.is_some();
            let disk = item.disk(target.root);
            let eval = evaluated(&ctx, &synced, target.root, &item)?;
            let what = match (&eval.state, exact) {
                (State::New, _) => "adopt",
                (State::ProjectChanged, _) => "copy into the mirror",
                (State::DeletedInMirror | State::MirrorChanged | State::Conflict, true) => {
                    "keep the project's version of"
                }
                (State::InSync, _) => continue,
                (State::DeletedInMirror | State::MirrorChanged | State::Conflict, false) => {
                    continue;
                }
                (state, _) => {
                    skips.add(
                        exact,
                        format!("skip {}: {}", disk.display(), describe(state)),
                    );
                    continue;
                }
            };
            planned.push(Planned {
                root: target.root,
                item,
                eval,
                what,
            });
        }
    }
    drop_tracked(&mut planned, &named, &mut skips);
    planned.retain(|p| {
        let content = p
            .eval
            .disk
            .as_ref()
            .expect("a planned adopt has read the project side");
        match check_rules(&ctx.repo, &p.item, content) {
            Ok(()) => true,
            Err(violations) => {
                let disk = p.item.disk(p.root);
                skips.add(
                    named.contains(&disk),
                    format!("skip {}: it breaks the rules:", disk.display()),
                );
                skips
                    .lines
                    .extend(violations.iter().map(|v| format!("  {v}")));
                if let Some(hint) = rules::allow_hint(&violations) {
                    skips.lines.push(hint);
                }
                false
            }
        }
    });
    let own = if skips.named {
        Exit::Refused
    } else {
        Exit::Success
    };
    if !show_plan(&planned, &skips, "adopt", yes, dry_run)? {
        return Ok(own);
    }
    let mut backups = Backups::new(&ctx.paths);
    for message in swap::recover(&swap_parent(&ctx.repo), Place::Mirror, &mut backups) {
        println!("{message}");
    }
    let mut exit = own;
    let mut written = Vec::new();
    for p in &planned {
        let content = p.eval.disk.as_ref().expect("read");
        let mirror = ctx.repo.root().join(p.item.mirror_rel());
        let outcome = swap::replace(
            &mirror,
            p.item.kind,
            &Expect::from_hash(p.eval.mirror_hash.as_deref()),
            content,
            &swap_parent(&ctx.repo),
            Place::Mirror,
            &mut backups,
            &Path::new("repo").join(p.item.mirror_rel()),
        );
        let replaced = matches!(outcome, Ok(Outcome::Replaced { .. }));
        exit = exit.combine(report_outcome(outcome, "adopted", &p.item.disk(p.root)));
        if replaced {
            synced.set(p.root, &p.item.rel, p.item.kind, &content.hash());
            written.push(p.item.mirror_rel());
        }
    }
    for (path, rule) in ignored(ctx.repo.root(), &written) {
        eprintln!(
            "warning: git ignores {path} in the skill repo ({rule}), so it never gets history; change that rule"
        );
    }
    finish_write(&ctx, &mut synced, &backups, exit)
}

/// Why an item is not adopted or restored, in a few words.
fn describe(state: &State) -> String {
    match state {
        State::MirrorInvalid(why) => format!("its mirror cannot be used: {why}"),
        State::ProjectGone => "its project folder is gone".to_owned(),
        State::NotHere => "its project folder is not on this machine".to_owned(),
        State::Occupied(what) => what.clone(),
        State::NotOnMachine => {
            "it is not in the project yet (`skiman project restore` writes it)".to_owned()
        }
        State::DeletedInProject => {
            "it was deleted in the project (name it exactly to restore it)".to_owned()
        }
        State::DeletedInMirror => {
            "it was deleted in the mirror (name it exactly to adopt it again)".to_owned()
        }
        State::Blocked(why) => format!("the project's version cannot be taken: {why}"),
        State::MirrorChanged => "its mirror changed (name it exactly to choose a side)".to_owned(),
        State::Conflict => "both sides changed (name it exactly to choose a side)".to_owned(),
        State::ProjectChanged => {
            "the project's version is newer; `skiman sync` copies it into the mirror".to_owned()
        }
        State::InSync => "both sides are equal".to_owned(),
        State::New => "it is not adopted yet".to_owned(),
        State::Forgotten | State::Nothing => "there is nothing on either side".to_owned(),
    }
}

// ---------------------------------------------------------------- restore

pub fn restore(paths: Vec<PathBuf>, dry_run: bool, yes: bool) -> Exit {
    finish(run_restore(paths, dry_run, yes))
}

fn run_restore(given: Vec<PathBuf>, dry_run: bool, yes: bool) -> Result<Exit, Failure> {
    need_yes(yes, dry_run)?;
    let ctx = Context::open()?;
    let mut synced = Synced::load(&ctx.paths);
    let targets = targets(&ctx, &given)?;
    let mut named = BTreeSet::new();
    let mut planned: Vec<Planned> = Vec::new();
    let mut skips = Skips::default();
    let mut seen = BTreeSet::new();
    for target in &targets {
        let items = known_items(&ctx, &synced, target)?;
        if let Some(exact) = &target.exact {
            named.insert(exact.disk(target.root));
            if items.is_empty() {
                return Err(Failure::refused(format!(
                    "{} is not in the mirror",
                    exact.disk(target.root).display()
                )));
            }
        }
        for item in items {
            if !seen.insert((item.root.clone(), item.rel.clone())) {
                continue;
            }
            let exact = target.exact.is_some();
            let disk = item.disk(target.root);
            let eval = evaluated(&ctx, &synced, target.root, &item)?;
            let what = match (&eval.state, exact) {
                (State::NotOnMachine, _) => "write",
                (
                    State::DeletedInProject
                    | State::Blocked(_)
                    | State::MirrorChanged
                    | State::Conflict,
                    true,
                ) => "write the mirror's version over",
                (State::InSync, _) => continue,
                (
                    State::DeletedInProject
                    | State::Blocked(_)
                    | State::MirrorChanged
                    | State::Conflict,
                    false,
                ) => {
                    continue;
                }
                (State::New | State::NotHere | State::Nothing | State::Forgotten, false) => {
                    continue;
                }
                (state, _) => {
                    skips.add(
                        exact,
                        format!("skip {}: {}", disk.display(), describe(state)),
                    );
                    continue;
                }
            };
            planned.push(Planned {
                root: target.root,
                item,
                eval,
                what,
            });
        }
    }
    drop_tracked(&mut planned, &named, &mut skips);
    let mut contents = BTreeMap::new();
    planned.retain(|p| {
        let disk = p.item.disk(p.root);
        let mirror = ctx.repo.root().join(p.item.mirror_rel());
        let content = match content::read(&mirror, p.item.kind) {
            Ok(Side::Present(content)) => content,
            _ => {
                skips.add(
                    named.contains(&disk),
                    format!("skip {}: its mirror cannot be read", disk.display()),
                );
                return false;
            }
        };
        if let Err(violations) = check_rules(&ctx.repo, &p.item, &content) {
            skips.add(
                named.contains(&disk),
                format!("skip {}: its mirror breaks the rules:", disk.display()),
            );
            skips
                .lines
                .extend(violations.iter().map(|v| format!("  {v}")));
            return false;
        }
        contents.insert(disk, content);
        true
    });
    let own = if skips.named {
        Exit::Refused
    } else {
        Exit::Success
    };
    if !show_plan(&planned, &skips, "restore", yes, dry_run)? {
        return Ok(own);
    }
    let mut backups = Backups::new(&ctx.paths);
    let mut exit = own;
    let mut recovered = BTreeSet::new();
    for p in &planned {
        let disk = p.item.disk(p.root);
        let anchor = p.item.anchor_dir(p.root);
        if recovered.insert(anchor.clone()) {
            for message in swap::recover(&anchor, Place::Project, &mut backups) {
                println!("{message}");
            }
        }
        let content = &contents[&disk];
        let expect = match &p.eval.state {
            State::Blocked(_) => Expect::Present,
            _ => Expect::from_hash(p.eval.disk_hash.as_deref()),
        };
        let outcome = swap::replace(
            &disk,
            p.item.kind,
            &expect,
            content,
            &anchor,
            Place::Project,
            &mut backups,
            &Path::new(MIRROR_DIR).join(&p.root.id).join(&p.item.rel),
        );
        let replaced = matches!(outcome, Ok(Outcome::Replaced { .. }));
        exit = exit.combine(report_outcome(outcome, "restored", &disk));
        if replaced {
            synced.set(p.root, &p.item.rel, p.item.kind, &content.hash());
        }
    }
    finish_write(&ctx, &mut synced, &backups, exit)
}

// ---------------------------------------------------------------- eject

pub fn eject(paths: Vec<PathBuf>, yes: bool) -> Exit {
    finish(run_eject(paths, yes))
}

/// A root as eject sees it: configured, whether or not it is usable here.
struct Configured {
    id: String,
    /// The `path` expanded lexically, and its realpath when it exists.
    paths: Vec<PathBuf>,
    skill_dirs: Vec<String>,
}

fn run_eject(given: Vec<PathBuf>, yes: bool) -> Result<Exit, Failure> {
    need_yes(yes, false)?;
    let ctx = Context::open()?;
    let home = ctx.paths.home();
    let configured: Vec<Configured> = ctx
        .manifest
        .projects()
        .into_iter()
        .map(|entry| {
            let lexical = entry.path.as_deref().and_then(|p| root::expand(home, p));
            let mut paths: Vec<PathBuf> = lexical.iter().cloned().collect();
            if let Some(real) = lexical.and_then(|p| p.canonicalize().ok()) {
                paths.push(real);
            }
            Configured {
                id: entry.id,
                paths,
                skill_dirs: entry.skill_dirs,
            }
        })
        .collect();
    let tree = WorkingTree::new(ctx.repo.root());
    let mut synced = Synced::load(&ctx.paths);
    // (root id, rel of the target, mirror items under it)
    let mut plan: Vec<(&Configured, String, Vec<Item>)> = Vec::new();
    for path in &given {
        let lexical = cwd_absolute(path)?;
        let real = realpath_as_far_as_exists(&lexical);
        let found = configured.iter().find_map(|c| {
            c.paths.iter().find_map(|root| {
                [&lexical, &real]
                    .into_iter()
                    .find_map(|p| p.strip_prefix(root).ok())
                    .map(|rel| (c, rel.to_string_lossy().into_owned()))
            })
        });
        let Some((configured, rel)) = found else {
            return Err(Failure::refused(format!(
                "{} is not inside a project root",
                path.display()
            )));
        };
        let mirror =
            read_mirror_of(&tree, &configured.id, &configured.skill_dirs).map_err(|e| {
                Failure::operational(format!("cannot read {}: {e}", ctx.repo.root().display()))
            })?;
        let items: Vec<Item> = mirror
            .items
            .into_iter()
            .filter(|i| under(&i.rel, &rel))
            .collect();
        plan.push((configured, rel, items));
    }
    let count: usize = plan.iter().map(|(_, _, items)| items.len()).sum();
    for (configured, rel, items) in &plan {
        for item in items {
            println!(
                "stop managing {} (its mirror goes to a backup; the project file stays)",
                item.mirror_rel()
            );
        }
        if rel.is_empty() {
            println!("remove [projects.{}] from skiman.toml", configured.id);
        }
    }
    let unregisters = plan.iter().any(|(_, rel, _)| rel.is_empty());
    if count == 0 && !unregisters {
        println!("nothing to eject");
    }
    if !yes && (count > 0 || unregisters) && !confirm("Eject? [y/N] ")? {
        return Err(Failure::refused("not ejected; nothing was changed"));
    }
    let mut backup: Option<Backup> = None;
    let mut exit = Exit::Success;
    let mut unregister = Vec::new();
    for (configured, rel, items) in &plan {
        let mut all_out = true;
        for item in items {
            let mirror = ctx.repo.root().join(item.mirror_rel());
            let backup = match &backup {
                Some(b) => b,
                None => backup.insert(Backup::create(&ctx.paths).map_err(|e| {
                    Failure::operational(format!(
                        "cannot create a backup folder: {e}; nothing was changed"
                    ))
                })?),
            };
            match backup.move_in(&mirror, &Path::new("repo").join(item.mirror_rel())) {
                Ok(to) => {
                    println!("moved {} to {}", item.mirror_rel(), to.display());
                    remove_empty_parents(&mirror, &ctx.repo.root().join(MIRROR_DIR));
                }
                Err(e) => {
                    eprintln!("error: cannot move {} to the backup: {e}", mirror.display());
                    exit = exit.combine(Exit::OperationalError);
                    all_out = false;
                }
            }
        }
        synced.remove_under(&configured.id, rel);
        if rel.is_empty() && all_out {
            unregister.push(configured.id.clone());
        }
    }
    if !unregister.is_empty() {
        let file = ctx.repo.manifest_file();
        let mut text = read_text(&file)?;
        for id in &unregister {
            text = manifest::without_project(&text, id)
                .map_err(|e| Failure::refused(format!("{}: {e}", file.display())))?;
            synced.remove_root(id);
            let _ = std::fs::remove_dir(ctx.repo.root().join(MIRROR_DIR).join(id));
        }
        let _ = std::fs::remove_dir(ctx.repo.root().join(MIRROR_DIR));
        write_atomic(&file, text.as_bytes())
            .map_err(|e| Failure::operational(format!("cannot write {}: {e}", file.display())))?;
        for id in &unregister {
            println!("removed [projects.{id}] from skiman.toml");
        }
    }
    if let Some(backup) = &backup {
        println!("the mirrors are in {}", backup.dir().display());
    }
    let backups = Backups::new(&ctx.paths);
    finish_write(&ctx, &mut synced, &backups, exit)
}

/// Removes the empty folders between `path` and `stop` (exclusive).
fn remove_empty_parents(path: &Path, stop: &Path) {
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == stop || !d.starts_with(stop) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

// ---------------------------------------------------------------- paths

fn cwd_absolute(path: &Path) -> Result<PathBuf, Failure> {
    let cwd = std::env::current_dir()
        .map_err(|e| Failure::operational(format!("cannot read the current folder: {e}")))?;
    let home = Paths::from_env()?.home().to_path_buf();
    let path = match path.to_str().and_then(|p| p.strip_prefix("~/")) {
        Some(rest) => home.join(rest),
        None => path.to_path_buf(),
    };
    Ok(absolute_lexical(&cwd, &path))
}

/// The realpath of the deepest existing ancestor of `path`, joined with the rest.
fn realpath_as_far_as_exists(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(real) = current.canonicalize() {
            return rest.iter().rev().fold(real, |p: PathBuf, c| p.join(c));
        }
        match (current.file_name().map(|n| n.to_owned()), current.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                current = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn read_text(file: &Path) -> Result<String, Failure> {
    std::fs::read_to_string(file)
        .map_err(|e| Failure::operational(format!("cannot read {}: {e}", file.display())))
}
