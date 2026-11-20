//! `skiman project add|adopt|restore|eject` (project-scope.md "Commands"). One
//! rule for all four: a command given a folder never replaces existing content
//! or undoes a local deletion on either side; that needs the item's exact path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup::Backups;
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::commands::{confirm, has_terminal};
use crate::failure::{Failure, finish};
use crate::fsutil::{broken_chain, realpath_as_far_as_exists, write_atomic};
use crate::linked::Linked;
use crate::manifest::{self, Manifest};
use crate::paths::Paths;
use crate::project::command;
use crate::project::discover::{discover, never_entered};
use crate::project::git::{ignored, tracking_repos};
use crate::project::item::{Item, MIRROR_DIR, managed, read_mirror_of};
use crate::project::root::{self, Root};
use crate::project::state::{Action, Evaluated, State, Verb, check_rules, classify};
use crate::project::swap::{Expect, Outcome, Previous, Replace};
use crate::project::sync::swap_parent;
use crate::project::synced::Synced;
use crate::rules::{self, Scope};
use crate::staging::{PROJECT_PREFIX, Staging};
use crate::sync::{self, Output};
use crate::tree::{Overlay, WorkingTree, is_ignored};

// ---------------------------------------------------------------- add

pub fn add(path: &Path, id: Option<String>) -> Exit {
    finish(run_add(path, id))
}

fn run_add(path: &Path, id: Option<String>) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = Canon::from_config(&paths)?;
    let home = paths.home();
    let dir = cwd_absolute(path)?
        .canonicalize()
        .ok()
        .filter(|d| d.is_dir())
        .ok_or_else(|| Failure::refused(format!("{} is not an existing folder", path.display())))?;
    root::validate_dir(home, &repo, &dir).map_err(Failure::refused)?;
    let id = id.unwrap_or_else(|| root::default_id(&dir));
    if id.is_empty() {
        return Err(Failure::refused(format!(
            "the name of {} has no letters or digits to make a root id from; give one with --id",
            dir.display()
        )));
    }
    let _lock = CanonLock::wait(&paths, &repo)?;
    let manifest = Manifest::load(&repo)?;
    if manifest.projects().iter().any(|entry| entry.id == id) {
        return Err(Failure::refused(format!(
            "skiman.toml already has [projects.{id}]; pick another id with --id"
        )));
    }
    let file = repo.manifest_file();
    let text = manifest::with_project(&read_text(&file)?, &id, &root::written_form(home, &dir))
        .map_err(Failure::refused_in(&file))?;
    // The same validation every command applies to a root, on the new text.
    let candidate = Manifest::parse(&text).map_err(Failure::refused_in(&file))?;
    if let Some(Err(problem)) = root::resolve(home, &repo, &candidate)
        .into_iter()
        .find(|r| match r {
            Ok(root) => root.id == id,
            Err(problem) => problem.id == id,
        })
    {
        let message = if crate::names::is_valid(&id) {
            problem.message
        } else {
            format!("{}; give one with --id", problem.message)
        };
        return Err(Failure::refused(message));
    }
    let tree = WorkingTree::new(repo.root());
    let overlay = Overlay::new(
        &tree,
        vec![(rules::MANIFEST.to_owned(), text.clone().into_bytes())],
    );
    rules::validate_write(
        &overlay,
        &Scope {
            manifest: true,
            ..Scope::default()
        },
    )?;
    crate::test_hook::at("project", "write:skiman.toml");
    write_atomic(&file, text.as_bytes())
        .map_err(|e| Failure::operational(format!("cannot write {}: {e}", file.display())))?;
    out!("registered project root {id}: {}", dir.display());
    out!(
        "its project files will be copied into {}/{MIRROR_DIR}/{id}/ and pushed wherever the canon is pushed; see what it holds with: {} --dry-run",
        repo.root().display(),
        command("adopt", std::slice::from_ref(&dir))
    );
    Ok(Exit::Success)
}

// ---------------------------------------------------------------- shared

/// What a command works on: the lock, the configuration and the usable roots.
struct Context {
    paths: Paths,
    repo: Canon,
    lock: CanonLock,
    manifest: Manifest,
    roots: Vec<Root>,
    /// [`never_entered`]: discovery never enters these, and [`Self::locate`]
    /// refuses a path inside them.
    skip: [PathBuf; 2],
    /// The linked folders: items that overlap one are skipped.
    linked: Linked,
}

impl Context {
    /// Takes the lock and refuses (exit 4) while the canon is in the
    /// middle of a git operation.
    fn open() -> Result<Context, Failure> {
        let paths = Paths::from_env()?;
        let repo = Canon::from_config(&paths)?;
        let lock = CanonLock::wait(&paths, &repo)?;
        if let Some(operation) = crate::project::operation(&repo) {
            return Err(Failure::operational(format!(
                "{operation}; finish that first, so nothing is written into an old checkout or a half-merged tree"
            )));
        }
        let manifest = Manifest::load(&repo)?;
        let mut roots = Vec::new();
        for resolved in root::resolve(paths.home(), &repo, &manifest) {
            match resolved {
                Ok(root) => roots.push(root),
                Err(problem) => err!(
                    "warning: project root `{}` is skipped: {}",
                    problem.id,
                    problem.message
                ),
            }
        }
        let skip = never_entered(&paths, &repo);
        let linked = Linked::read(paths.home(), &repo, &manifest);
        Ok(Context {
            paths,
            repo,
            lock,
            manifest,
            roots,
            skip,
            linked,
        })
    }

    fn backups(&self) -> Backups {
        Backups::new(&self.paths)
    }

    /// The root a path given on the command line is in, and the path
    /// relative to it (`""` for the root itself). The parents are resolved
    /// as far as they exist; the last component is not, so a path that is an
    /// alias link names the link, not what it points to. A path in the canon
    /// or the state folder is refused: adopting a mirror would mirror
    /// it again.
    fn locate(&self, path: &Path) -> Result<(&Root, String), Failure> {
        let absolute = cwd_absolute(path)?;
        let real = realpath_of_parent(&absolute);
        if let Some(dir) = self.skip.iter().find(|dir| real.starts_with(dir)) {
            return Err(Failure::refused(format!(
                "{} is inside {}, which project commands never read or write; name the item's path in its project",
                path.display(),
                dir.display()
            )));
        }
        let whole = realpath_as_far_as_exists(&absolute);
        self.roots
            .iter()
            .find_map(|root| {
                let rel = real
                    .strip_prefix(&root.dir)
                    .ok()
                    .or_else(|| (whole == root.dir).then(|| Path::new("")))?;
                Some((root, rel.to_string_lossy().into_owned()))
            })
            .ok_or_else(|| {
                Failure::refused(format!(
                    "{} is not inside a project root (`skiman project add <folder>` registers one)",
                    path.display()
                ))
            })
    }

    fn classify(&self, synced: &Synced, root: &Root, item: &Item) -> Result<Evaluated, Failure> {
        let recorded = synced.get(root, &item.rel).map(|(_, h)| h.clone());
        classify(&self.repo, root, item, recorded.as_deref()).map_err(|e| {
            Failure::operational(format!("cannot read {}: {e}", item.disk(root).display()))
        })
    }

    /// Saves the synced hashes, reports the backup, and syncs. `own` is the
    /// exit status of the command's own work; `failed` the items whose write
    /// failed, which sync says nothing about.
    fn finish(
        &self,
        synced: &mut Synced,
        backups: &Backups,
        own: Exit,
        failed: &[PathBuf],
    ) -> Result<Exit, Failure> {
        synced.save(&self.paths).map_err(|e| {
            Failure::operational(format!(
                "cannot write {}: {e}",
                self.paths.state_dir().display()
            ))
        })?;
        if let Some(dir) = backups.created() {
            out!("replaced versions are in {}", dir.display());
        }
        let sync = sync::run_with(
            &self.paths,
            &self.repo,
            &self.lock,
            Output::Normal,
            None,
            failed,
        );
        Ok(own.combine(sync.0))
    }

    fn sync(&self) -> Exit {
        sync::run(&self.paths, &self.repo, &self.lock, Output::Normal)
    }
}

/// One path the user named, resolved.
struct Target<'a> {
    root: &'a Root,
    rel: String,
    /// The path names an item exactly.
    exact: Option<Item>,
}

/// The targets of `given`, items named exactly first, so an exact path is
/// never shadowed by a folder that holds it; every root when `given` is empty.
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
    let mut targets = given
        .iter()
        .map(|path| {
            let (root, rel) = ctx.locate(path)?;
            let is_dir = std::fs::symlink_metadata(root.dir.join(&rel)).is_ok_and(|m| m.is_dir());
            let exact = (!rel.is_empty())
                .then(|| Item::at(root, &rel, is_dir).or_else(|| Item::at(root, &rel, !is_dir)))
                .flatten();
            Ok(Target { root, rel, exact })
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    targets.sort_by_key(|t| t.exact.is_none());
    Ok(targets)
}

fn under(rel: &str, folder: &str) -> bool {
    folder.is_empty() || rel == folder || rel.starts_with(&format!("{folder}/"))
}

/// Refuses a folder target that holds nothing and does not exist (a typo).
fn refuse_missing(target: &Target, found_nothing: bool) -> Result<(), Failure> {
    let path = target.root.dir.join(&target.rel);
    if target.exact.is_none() && !target.rel.is_empty() && found_nothing && !path.is_dir() {
        return Err(Failure::refused(format!(
            "{} is neither a project item nor a folder holding one",
            path.display()
        )));
    }
    Ok(())
}

/// Refuses (exit 2) a command that would ask without a terminal.
fn need_yes(yes: bool, dry_run: bool) -> Result<(), Failure> {
    if !yes && !dry_run && !has_terminal() {
        return Err(Failure::usage("missing flag without a terminal: --yes"));
    }
    Ok(())
}

fn question(verb: Verb, count: usize) -> String {
    match verb {
        Verb::Adopt => format!("Adopt {count} item(s)? [y/N] "),
        Verb::Restore => format!("Restore {count} item(s)? [y/N] "),
    }
}

/// One planned write.
struct Planned<'a> {
    root: &'a Root,
    item: Item,
    /// Read once when the plan is made: the rules check exactly what is
    /// written ([`Evaluated::source`]), and the live source is not re-read.
    eval: Evaluated,
    /// What the plan line says is done to the item.
    what: &'static str,
}

/// The plan of a write command: the writes, and what is skipped and why.
struct Plan<'a> {
    verb: Verb,
    writes: Vec<Planned<'a>>,
    skips: Vec<String>,
    /// Paths the user named exactly.
    named: BTreeSet<PathBuf>,
    /// An item named exactly was skipped (exit 1).
    named_skipped: bool,
    /// An item under a folder could not be read, and was skipped (exit 4).
    unreadable: bool,
    /// The items already taken, so two given paths do not plan one twice.
    seen: BTreeSet<(String, String)>,
}

impl<'a> Plan<'a> {
    fn new(verb: Verb) -> Plan<'a> {
        Plan {
            verb,
            writes: Vec::new(),
            skips: Vec::new(),
            named: BTreeSet::new(),
            named_skipped: false,
            unreadable: false,
            seen: BTreeSet::new(),
        }
    }

    fn skip(&mut self, disk: &Path, line: String) {
        self.named_skipped |= self.named.contains(disk);
        self.skips.push(line);
    }

    /// Plans `items` of `target` by their state ([`Evaluated::action`]).
    fn take(
        &mut self,
        ctx: &Context,
        synced: &Synced,
        target: &Target<'a>,
        items: Vec<Item>,
    ) -> Result<(), Failure> {
        let exact = target.exact.is_some();
        for item in items {
            if !self.seen.insert((item.root.clone(), item.rel.clone())) {
                continue;
            }
            let disk = item.disk(target.root);
            if let Some(why) = ctx.linked.item_overlap(&disk) {
                self.skip(&disk, format!("skip {}: {why}", disk.display()));
                continue;
            }
            let eval = match ctx.classify(synced, target.root, &item) {
                Ok(eval) => eval,
                // Named exactly, it stops the command; under a folder the rest go on.
                Err(failure) if exact => return Err(failure),
                Err(failure) => {
                    self.skips.push(failure.message);
                    self.unreadable = true;
                    continue;
                }
            };
            let what = match (eval.action(self.verb), exact) {
                (Action::Write(what), _) | (Action::Exact(what), true) => what,
                (Action::Exact(_), false) => {
                    // A folder command leaves it alone: settling it needs the exact path.
                    self.skips.push(format!(
                        "skip {}: {}; to settle it, give its exact path: {}",
                        disk.display(),
                        eval.state.describe(),
                        command(self.verb.word(), std::slice::from_ref(&disk))
                    ));
                    continue;
                }
                (Action::Settled, _) | (Action::Elsewhere, false) => continue,
                (Action::Elsewhere | Action::Skip, _) => {
                    let line = format!("skip {}: {}", disk.display(), eval.state.describe());
                    self.skip(&disk, line);
                    continue;
                }
            };
            self.writes.push(Planned {
                root: target.root,
                item,
                eval,
                what,
            });
        }
        Ok(())
    }

    /// Gate 1: drops every write a project's own git repo tracks, or whose
    /// repo git cannot read.
    fn drop_tracked(&mut self) {
        let disks: Vec<PathBuf> = self.writes.iter().map(|p| p.item.disk(p.root)).collect();
        let tracking = tracking_repos(&disks);
        let mut by_repo: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        let mut skipped = Vec::new();
        self.writes.retain(|p| {
            let disk = p.item.disk(p.root);
            match tracking.get(&disk) {
                Some(Ok(Some(top))) => {
                    by_repo.entry(top.clone()).or_default().push(disk);
                    false
                }
                Some(Err(e)) => {
                    skipped.push((disk.clone(), format!("skip {}: {e}", disk.display())));
                    false
                }
                _ => true,
            }
        });
        for (disk, line) in skipped {
            self.skip(&disk, line);
        }
        for (top, disks) in by_repo {
            self.named_skipped |= disks.iter().any(|d| self.named.contains(d));
            self.skips.push(format!(
                "skip {} item(s) tracked by the git repo {} (that repo already keeps their history):",
                disks.len(),
                top.display()
            ));
            self.skips
                .extend(disks.iter().map(|d| format!("  {}", d.display())));
        }
    }

    /// Drops every write whose new content cannot be read or breaks the
    /// safety rules.
    fn drop_breaking(&mut self, repo: &Canon) {
        let source = match self.verb {
            Verb::Adopt => "the project's version",
            Verb::Restore => "its mirror",
        };
        let mut skipped = Vec::new();
        let verb = self.verb;
        self.writes.retain(|p| {
            let disk = p.item.disk(p.root);
            let Some(new) = p.eval.source(verb) else {
                skipped.push((
                    disk.clone(),
                    vec![format!("skip {}: {source} cannot be read", disk.display())],
                ));
                return false;
            };
            match check_rules(repo, &p.item, &new.content) {
                Ok(()) => true,
                Err(violations) => {
                    let mut lines = vec![format!(
                        "skip {}: {source} breaks the rules:",
                        disk.display()
                    )];
                    lines.extend(violations.iter().map(|v| format!("  {v}")));
                    lines.extend(rules::allow_hint(&violations));
                    skipped.push((disk, lines));
                    false
                }
            }
        });
        for (disk, lines) in skipped {
            self.named_skipped |= self.named.contains(&disk);
            self.skips.extend(lines);
        }
    }

    /// Prints the plan and the skips, and asks unless `yes`. `false` when
    /// there is nothing to write or it is a dry run.
    fn confirm(&self, yes: bool, dry_run: bool) -> Result<bool, Failure> {
        for p in &self.writes {
            out!("{} {}", p.what, p.item.disk(p.root).display());
        }
        for line in &self.skips {
            err!("{line}");
        }
        if self.writes.is_empty() {
            out!("nothing to {}", self.verb.word());
            return Ok(false);
        }
        if dry_run {
            return Ok(false);
        }
        if !yes && !confirm(&question(self.verb, self.writes.len()))? {
            return Err(Failure::refused(format!(
                "not {}; nothing was changed",
                self.verb.done()
            )));
        }
        Ok(true)
    }

    fn own_exit(&self) -> Exit {
        if self.unreadable {
            Exit::OperationalError
        } else if self.named_skipped {
            Exit::Refused
        } else {
            Exit::Success
        }
    }
}

/// Refuses (exit 3) to change project items while a linked folder of
/// `linked` cannot be read: its child links, and so what overlaps it, are
/// unknown. A git operation, the other reason of `paused_by`, never gets
/// here: [`Context::open`] refuses it first.
fn refuse_while_unreadable(repo: &Canon, linked: &Linked, verb: &str) -> Result<(), Failure> {
    match crate::project::paused_by(repo, linked) {
        Some(why) => Err(Failure {
            exit: Exit::NeedsAttention,
            message: format!("{why}; nothing was {verb}, run this again once it can be read"),
        }),
        None => Ok(()),
    }
}

/// The linked folders once the user has answered, from `skiman.toml` read
/// again: the lock stops other skiman commands, not a re-pointed path or a
/// hand edit. Refuses as [`refuse_while_unreadable`] does.
fn linked_after_answer(ctx: &Context, verb: &str) -> Result<Linked, Failure> {
    let linked = Linked::load(ctx.paths.home(), &ctx.repo)?;
    refuse_while_unreadable(&ctx.repo, &linked, verb)?;
    Ok(linked)
}

/// Reports the planned write to `disk` as skipped when it overlaps a folder
/// of `linked`, and returns the exit that adds: refused for a path named
/// exactly, as in the plan.
fn skip_overlapping(linked: &Linked, plan: &Plan, disk: &Path) -> Option<Exit> {
    let why = linked.item_overlap(disk)?;
    err!("skip {}: {why}", disk.display());
    Some(if plan.named.contains(disk) {
        Exit::Refused
    } else {
        Exit::Success
    })
}

/// Prints the outcome of one replace and returns the exit it adds; the
/// new content is in place exactly when the first value is `true`.
fn report_outcome(outcome: std::io::Result<Outcome>, verb: Verb, disk: &Path) -> (bool, Exit) {
    match outcome {
        Ok(Outcome::Replaced {
            previous,
            late_write,
        }) => {
            out!("{} {}", verb.done(), disk.display());
            match previous {
                Previous::At(backup) if late_write => {
                    err!(
                        "warning: a write landed while {} was replaced; that version is in {}",
                        disk.display(),
                        backup.display()
                    );
                    (true, Exit::NeedsAttention)
                }
                Previous::Kept(why) => {
                    err!("error: {} is {}, but {why}", disk.display(), verb.done());
                    (true, Exit::OperationalError)
                }
                _ => (true, Exit::Success),
            }
        }
        Ok(Outcome::Conflict(why)) => {
            err!("warning: {why}; nothing was replaced, run the command again");
            (false, Exit::NeedsAttention)
        }
        Ok(Outcome::Failed(why)) => {
            err!("error: {why}");
            (false, Exit::OperationalError)
        }
        Err(e) => {
            err!("error: cannot write {}: {e}", disk.display());
            (false, Exit::OperationalError)
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
    refuse_while_unreadable(&ctx.repo, &ctx.linked, "adopted")?;
    let synced = Synced::load(&ctx.paths);
    let targets = targets(&ctx, &given)?;
    let mut plan = Plan::new(Verb::Adopt);
    for target in &targets {
        let candidates: Vec<Item> = match &target.exact {
            Some(item) => {
                plan.named.insert(item.disk(target.root));
                vec![item.clone()]
            }
            None => {
                let found = discover(target.root, &ctx.skip, &ctx.linked, &target.rel);
                for (item, why) in found
                    .skipped
                    .iter()
                    .filter(|(i, _)| under(&i.rel, &target.rel))
                {
                    let disk = item.disk(target.root);
                    plan.skip(&disk, format!("skip {}: {why}", disk.display()));
                    plan.seen.insert((item.root.clone(), item.rel.clone()));
                }
                for e in found.errors {
                    err!("warning: {e}");
                }
                for leftover in found.swap_leftovers {
                    err!(
                        "warning: {} is left from an earlier `skiman project restore`; the next restore there moves it to a backup",
                        leftover.display()
                    );
                }
                // The items only the mirror knows (deleted in the project or
                // in the mirror, changed there) are listed with their fix too.
                let mut items = found.items;
                let tree = WorkingTree::new(ctx.repo.root());
                for item in managed(&tree, target.root, &synced).map_err(io_failure)? {
                    if !items.contains(&item) {
                        items.push(item);
                    }
                }
                items.retain(|i| under(&i.rel, &target.rel));
                items
            }
        };
        refuse_missing(target, candidates.is_empty())?;
        plan.take(&ctx, &synced, target, candidates)?;
    }
    write(&ctx, plan, synced, yes, dry_run)
}

// ---------------------------------------------------------------- restore

pub fn restore(paths: Vec<PathBuf>, dry_run: bool, yes: bool) -> Exit {
    finish(run_restore(paths, dry_run, yes))
}

fn run_restore(given: Vec<PathBuf>, dry_run: bool, yes: bool) -> Result<Exit, Failure> {
    need_yes(yes, dry_run)?;
    let ctx = Context::open()?;
    refuse_while_unreadable(&ctx.repo, &ctx.linked, "restored")?;
    let synced = Synced::load(&ctx.paths);
    let targets = targets(&ctx, &given)?;
    let tree = WorkingTree::new(ctx.repo.root());
    let mut plan = Plan::new(Verb::Restore);
    for target in &targets {
        let items: Vec<Item> = managed(&tree, target.root, &synced)
            .map_err(io_failure)?
            .into_iter()
            .filter(|i| match &target.exact {
                Some(exact) => i.rel == exact.rel,
                None => under(&i.rel, &target.rel),
            })
            .collect();
        refuse_missing(target, items.is_empty())?;
        if let Some(exact) = &target.exact {
            plan.named.insert(exact.disk(target.root));
            if items.is_empty() {
                return Err(Failure::refused(format!(
                    "{} is not in the mirror",
                    exact.disk(target.root).display()
                )));
            }
        }
        plan.take(&ctx, &synced, target, items)?;
    }
    write(&ctx, plan, synced, yes, dry_run)
}

// ---------------------------------------------------------------- write

/// Gates `plan`, prints it, asks, and writes each item: `adopt` into the
/// mirror, `restore` into the project. Records each written item's synced
/// hash and ends with sync; `adopt` also when nothing is written, unless it
/// is a dry run.
fn write(
    ctx: &Context,
    mut plan: Plan,
    mut synced: Synced,
    yes: bool,
    dry_run: bool,
) -> Result<Exit, Failure> {
    let verb = plan.verb;
    plan.drop_tracked();
    plan.drop_breaking(&ctx.repo);
    let mut exit = plan.own_exit();
    if !plan.confirm(yes, dry_run)? {
        return Ok(match verb {
            Verb::Adopt if !dry_run => exit.combine(ctx.sync()),
            _ => exit,
        });
    }
    let linked = linked_after_answer(ctx, verb.done())?;
    let mut backups = ctx.backups();
    let (mut written, mut failed) = (Vec::new(), Vec::new());
    let (swap, saved) = swap_parent(&ctx.repo);
    for p in &plan.writes {
        let disk = p.item.disk(p.root);
        if let Some(refused) = skip_overlapping(&linked, &plan, &disk) {
            exit = exit.combine(refused);
            continue;
        }
        let new = p
            .eval
            .source(verb)
            .expect("drop_breaking keeps only readable writes");
        let say = |l| out!("{l}");
        // One staging per replace, so a command killed between two leaves none.
        let (dest, staging, expected, backup_rel) = match verb {
            Verb::Adopt => {
                let rel = p.item.mirror_rel();
                let root = ctx.repo.root();
                let mirror = p.eval.mirror.as_ref().map(|m| m.hash.as_str());
                (
                    root.join(&rel),
                    Staging::open(&swap, "", root, &mut backups, saved, say),
                    Expect::from_hash(mirror),
                    Path::new("repo").join(&rel),
                )
            }
            Verb::Restore => {
                // Its tmp leftovers go where the anchor's items go in the backup.
                let items = Path::new(MIRROR_DIR).join(&p.root.id);
                let (anchor, root) = (p.item.anchor_dir(p.root), &p.root.dir);
                let saved = items.join(&p.item.anchor);
                let expected = match (&p.eval.state, &p.eval.disk) {
                    (State::Blocked(_), None) => Expect::Present,
                    (_, disk) => Expect::from_hash(disk.as_ref().map(|d| d.hash.as_str())),
                };
                (
                    disk.clone(),
                    Staging::open(&anchor, PROJECT_PREFIX, root, &mut backups, &saved, say),
                    expected,
                    items.join(&p.item.rel),
                )
            }
        };
        let replace = Replace {
            dest: &dest,
            kind: p.item.kind,
            expected: &expected,
            new: &new.content,
            backup_rel: &backup_rel,
        };
        // A staging that cannot be opened fails this item only.
        let outcome = staging.and_then(|staging| replace.run(&staging, &mut backups));
        let (placed, more) = report_outcome(outcome, verb, &disk);
        exit = exit.combine(more);
        if more == Exit::OperationalError {
            failed.push(disk);
        }
        if placed {
            synced.set(p.root, &p.item.rel, p.item.kind, &new.hash);
            written.push(p.item.mirror_rel());
        }
    }
    if verb == Verb::Adopt {
        for (path, rule) in ignored(ctx.repo.root(), &written) {
            err!(
                "warning: git ignores {path} in the canon ({rule}), so it never gets history; change that rule"
            );
        }
    }
    ctx.finish(&mut synced, &backups, exit, &failed)
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
}

/// What eject does under one path.
struct Ejection<'a> {
    root: &'a Configured,
    rel: String,
    /// Mirror items under the path.
    items: Vec<Item>,
    /// Synced hashes under the path whose mirror is already gone.
    forgotten: Vec<String>,
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
            }
        })
        .collect();
    let tree = WorkingTree::new(ctx.repo.root());
    let mut synced = Synced::load(&ctx.paths);
    let mut plan: Vec<Ejection> = Vec::new();
    for path in &given {
        let lexical = cwd_absolute(path)?;
        let real = realpath_of_parent(&lexical);
        let found = configured.iter().find_map(|c| {
            c.paths.iter().find_map(|root| {
                [&lexical, &real]
                    .into_iter()
                    .find_map(|p| p.strip_prefix(root).ok())
                    .map(|rel| (c, rel.to_string_lossy().into_owned()))
            })
        });
        let Some((root, rel)) = found else {
            return Err(Failure::refused(format!(
                "{} is not inside a project root",
                path.display()
            )));
        };
        let items: Vec<Item> = read_mirror_of(&tree, &root.id)
            .map_err(io_failure)?
            .items
            .into_iter()
            .filter(|i| under(&i.rel, &rel))
            .collect();
        let forgotten = synced
            .rels_under(&root.id, &rel)
            .into_iter()
            .filter(|r| !items.iter().any(|i| &i.rel == r))
            .collect();
        plan.push(Ejection {
            root,
            rel,
            items,
            forgotten,
        });
    }
    let mut anything = false;
    for ejection in &plan {
        for item in &ejection.items {
            out!(
                "stop managing {} (its mirror goes to a backup; the project file stays)",
                item.mirror_rel()
            );
        }
        for rel in &ejection.forgotten {
            out!(
                "forget {rel} in project root {} (its mirror is already gone)",
                ejection.root.id
            );
        }
        if ejection.rel.is_empty() {
            out!("remove [projects.{}] from skiman.toml", ejection.root.id);
        }
        anything |=
            !ejection.items.is_empty() || !ejection.forgotten.is_empty() || ejection.rel.is_empty();
    }
    if !anything {
        out!("nothing to eject");
        return Ok(Exit::Success);
    }
    if !yes && !confirm("Eject? [y/N] ")? {
        return Err(Failure::refused("not ejected; nothing was changed"));
    }
    let mut backups = ctx.backups();
    let mut exit = Exit::Success;
    let mut unregister = Vec::new();
    let projects_dir = ctx.repo.root().join(MIRROR_DIR);
    for ejection in &plan {
        let mut all_out = true;
        for item in &ejection.items {
            let mirror = ctx.repo.root().join(item.mirror_rel());
            // Never move or clean up through a link: it could lead out of the repo.
            if let Some(why) = broken_chain(ctx.repo.root(), &mirror) {
                err!(
                    "error: {} is not ejected: its mirror cannot be used: {why}",
                    item.mirror_rel()
                );
                exit = exit.combine(Exit::NeedsAttention);
                all_out = false;
                continue;
            }
            crate::test_hook::at("project", &format!("eject:{}", item.mirror_rel()));
            match backups.move_in(&mirror, &Path::new("repo").join(item.mirror_rel())) {
                Ok(to) => {
                    out!("moved {} to {}", item.mirror_rel(), to.display());
                    synced.remove_under(&ejection.root.id, &item.rel);
                    remove_empty_parents(&mirror, &projects_dir);
                }
                Err(e) => {
                    err!("error: cannot move {} to the backup: {e}", mirror.display());
                    exit = exit.combine(Exit::OperationalError);
                    all_out = false;
                }
            }
        }
        // Only what is out of the mirror is forgotten: an item that stayed
        // keeps its synced hash, so its next project change is no conflict.
        // A mirror that only looks gone because a link hides it is not.
        for rel in &ejection.forgotten {
            let mirror = projects_dir.join(&ejection.root.id).join(rel);
            if let Some(why) = broken_chain(ctx.repo.root(), &mirror) {
                err!(
                    "error: {MIRROR_DIR}/{}/{rel} is not forgotten: its mirror cannot be used: {why}",
                    ejection.root.id
                );
                exit = exit.combine(Exit::NeedsAttention);
                all_out = false;
                continue;
            }
            synced.remove_under(&ejection.root.id, rel);
        }
        if ejection.rel.is_empty() && all_out {
            let id = &ejection.root.id;
            // The same no-link rule as for every mirror item.
            match broken_chain(ctx.repo.root(), &projects_dir.join(id).join("_")) {
                Some(why) => {
                    err!("error: [projects.{id}] is not removed: {why}");
                    exit = exit.combine(Exit::NeedsAttention);
                }
                None => unregister.push(id.clone()),
            }
        }
    }
    if !unregister.is_empty() {
        let file = ctx.repo.manifest_file();
        let mut text = read_text(&file)?;
        for id in &unregister {
            text = manifest::without_project(&text, id).map_err(Failure::refused_in(&file))?;
            synced.remove_root(id);
            remove_if_empty(&projects_dir.join(id));
        }
        remove_if_empty(&projects_dir);
        crate::test_hook::at("project", "write:skiman.toml");
        write_atomic(&file, text.as_bytes())
            .map_err(|e| Failure::operational(format!("cannot write {}: {e}", file.display())))?;
        for id in &unregister {
            out!("removed [projects.{id}] from skiman.toml");
        }
    }
    if let Some(dir) = backups.created() {
        out!("the mirrors are in {}", dir.display());
    }
    ctx.finish(&mut synced, &ctx.backups(), exit, &[])
}

/// Removes `dir` when it holds nothing but empty folders and files with an
/// ignored name ([`crate::tree::IGNORED_NAMES`]).
fn remove_if_empty(dir: &Path) {
    // A link is never followed: what it points to is not the repo's.
    if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => remove_if_empty(&path),
            Ok(_) if is_ignored(&entry.file_name().to_string_lossy()) => {
                let _ = std::fs::remove_file(&path);
            }
            _ => {}
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Removes the folders between `path` and `stop` (exclusive) that hold
/// nothing but ignored files.
fn remove_empty_parents(path: &Path, stop: &Path) {
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == stop || !d.starts_with(stop) {
            break;
        }
        remove_if_empty(d);
        if d.exists() {
            break;
        }
        dir = d.parent();
    }
}

// ---------------------------------------------------------------- paths

fn io_failure(e: std::io::Error) -> Failure {
    Failure::operational(format!("cannot read the canon: {e}"))
}

fn cwd_absolute(path: &Path) -> Result<PathBuf, Failure> {
    let cwd = std::env::current_dir()
        .map_err(|e| Failure::operational(format!("cannot read the current folder: {e}")))?;
    Ok(root::given(Paths::from_env()?.home(), &cwd, path))
}

/// `path` with its parent folder resolved (as far as it exists) and its last
/// component kept as written.
fn realpath_of_parent(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => realpath_as_far_as_exists(parent).join(name),
        _ => realpath_as_far_as_exists(path),
    }
}

fn read_text(file: &Path) -> Result<String, Failure> {
    std::fs::read_to_string(file)
        .map_err(|e| Failure::operational(format!("cannot read {}: {e}", file.display())))
}
