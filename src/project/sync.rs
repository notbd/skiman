//! The project part of `skiman sync` (project-scope.md "Sync: the state
//! table"): per mirror item and per synced-hash entry, never by walking the
//! roots. Sync writes only the mirror (row 13) and machine state; it never
//! writes a project folder and never spawns git.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::command;
use super::content::Content;
use super::item::{Item, MIRROR_DIR, managed};
use super::root::{self, Root};
use super::state::{State, classify};
use super::swap::{Expect, Outcome, Previous, Replace};
use super::synced::Synced;
use crate::backup::Backups;
use crate::canon::Canon;
use crate::linked::Linked;
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::staging::Staging;
use crate::sync::{Change, Item as Report, ItemKind, Output, Report as SyncReport};
use crate::tree::WorkingTree;

/// The roots [`root::resolve`] gives: usable, or why not.
pub type Roots = [Result<Root, root::Problem>];

/// Reconciles every project root into `report`: `roots` from
/// [`root::resolve`] when the caller has them (doctor), else resolved here
/// once nothing pauses project sync. An item that overlaps a linked folder
/// is skipped; while a linked folder cannot be read, or a git operation
/// runs in the canon, nothing is (`crate::linked`). For [`Output::Plan`]
/// nothing is written: a mirror write is only a change in the report, and
/// the synced hashes are not saved; a pause does not stop it judging the
/// items, read-only, and the roots that cannot be used are not reported
/// (doctor says what each one needs).
pub fn reconcile(
    paths: &Paths,
    repo: &Canon,
    (manifest, linked, roots): (&Manifest, &Linked, Option<&Roots>),
    output: Output,
    report: &mut SyncReport,
) {
    if manifest.projects().is_empty() {
        return;
    }
    let scope = repo.root().join(MIRROR_DIR).display().to_string();
    let plan = output == Output::Plan;
    let unreadable = linked.unreadable();
    if let Some(folder) = unreadable {
        report.items.push(note(
            &scope,
            repo.root().join(MIRROR_DIR),
            "paused",
            format!(
                "project items are not synced while linked folder `{}` cannot be read, since its child links are unknown; run `skiman sync` once it can be read",
                folder.id
            ),
            false,
        ));
        if !plan {
            return;
        }
        // A plan still says what it sees of the items, read-only.
        report.items.push(note(
            &scope,
            repo.root().join(MIRROR_DIR),
            "paused-seen",
            format!(
                "any project items listed below are as they are now; which of them overlap linked folder `{}` is known, and their fixes work, once it can be read",
                folder.id
            ),
            false,
        ));
    }
    // A plan still says what the items need: the user settles them anyway.
    let operation = super::operation(repo);
    if let Some(operation) = &operation {
        report.items.push(note(
            &scope,
            repo.root().join(MIRROR_DIR),
            "paused",
            format!(
                "{operation}, so project items are not synced; finish that, then run `skiman sync`"
            ),
            false,
        ));
        if !plan {
            return;
        }
    }
    let mut synced = Synced::load(paths);
    let configured: Vec<String> = manifest.projects().into_iter().map(|p| p.id).collect();
    synced.forget_unconfigured(&configured);
    let mut writer = Writer {
        repo,
        backups: Backups::new(paths),
        blocked: false,
        plan,
        paused: operation.is_some() || unreadable.is_some(),
    };
    let resolved_here;
    let roots = match roots {
        Some(roots) => roots,
        None => {
            resolved_here = root::resolve(paths.home(), repo, manifest);
            &resolved_here
        }
    };
    for resolved in roots {
        match resolved {
            Err(_) if plan => {}
            Err(problem) => report.items.push(note(
                &scope,
                repo.root().join(MIRROR_DIR).join(&problem.id),
                "root",
                problem.skipped(),
                false,
            )),
            Ok(root) => reconcile_root(root, linked, &mut synced, &mut writer, report),
        }
    }
    if !writer.plan
        && let Err(e) = synced.save(paths)
    {
        report
            .errors
            .push(format!("cannot write {}: {e}", paths.state_dir().display()));
    }
}

/// The paths of one root's items that need one command each, collected so
/// sync can also print one line for all of them.
#[derive(Default)]
struct Pending {
    not_here: usize,
    /// Row 7, which sync is silent on; [`Output::Plan`] counts them.
    away: usize,
    deleted_in_project: Vec<PathBuf>,
    deleted_in_mirror: Vec<PathBuf>,
    changed: Vec<PathBuf>,
}

fn reconcile_root(
    root: &Root,
    linked: &Linked,
    synced: &mut Synced,
    writer: &mut Writer,
    report: &mut SyncReport,
) {
    let repo = writer.repo;
    let scope = root.dir.display().to_string();
    let items = match managed(&WorkingTree::new(repo.root()), root, synced) {
        Ok(items) => items,
        Err(e) => {
            report.errors.push(format!(
                "cannot read {}/{MIRROR_DIR}/{}: {e}",
                repo.root().display(),
                root.id
            ));
            return;
        }
    };
    // Row 3: a recorded path that no longer has its item's shape.
    for (rel, _) in synced.items(root) {
        if !items.iter().any(|i| i.rel == rel) {
            synced.remove(root, &rel);
        }
    }
    let mut gone_anchors = BTreeSet::new();
    let mut pending = Pending::default();
    for item in &items {
        // Skipped, not forgotten: its synced hash stays.
        if let Some(why) = linked.item_overlap(&item.disk(root)) {
            let disk = item.disk(root);
            let message = format!("{} is not synced: {why}", disk.display());
            report
                .items
                .push(note(&scope, disk.clone(), "linked", message, false));
            // While paused a plan still says what it sees of both sides, read-only;
            // no `project adopt` or `restore` takes an item that overlaps.
            if writer.plan && writer.paused {
                let recorded = synced.get(root, &item.rel).map(|(_, h)| h.clone());
                match classify(repo, root, item, recorded.as_deref()) {
                    Ok(e) if e.state == State::Conflict => {
                        let message = format!("{}: it and its mirror both changed", disk.display());
                        report
                            .items
                            .push(note(&scope, disk, "conflict", message, true));
                    }
                    Ok(_) => {}
                    Err(e) => report
                        .errors
                        .push(format!("cannot read {}: {e}", disk.display())),
                }
            }
            continue;
        }
        let recorded = synced.get(root, &item.rel).map(|(_, h)| h.clone());
        let evaluated = match classify(repo, root, item, recorded.as_deref()) {
            Ok(evaluated) => evaluated,
            Err(e) => {
                report
                    .errors
                    .push(format!("cannot read {}: {e}", item.disk(root).display()));
                continue;
            }
        };
        let disk = item.disk(root);
        let say = |report: &mut SyncReport, what: &'static str| {
            report.items.push(note(
                &scope,
                disk.clone(),
                what,
                evaluated.message(root, item),
                evaluated.state.needs_attention(),
            ));
        };
        match &evaluated.state {
            State::NotHere => pending.away += 1,
            State::Nothing | State::New => {}
            State::InSync => {
                if let Some(hash) = &evaluated.disk_hash {
                    synced.set(root, &item.rel, item.kind, hash);
                }
            }
            State::NotOnMachine => pending.not_here += 1,
            // Sync names a gone folder once; a plan names each item, as doctor did.
            State::ProjectGone => {
                if gone_anchors.insert(item.anchor_dir(root)) || writer.plan {
                    say(report, "gone");
                }
            }
            State::MirrorInvalid(_) => say(report, "mirror-invalid"),
            State::Occupied(_) => say(report, "occupied"),
            State::Blocked(_) => say(report, "blocked"),
            State::DeletedInProject => {
                say(report, "deleted-in-project");
                pending.deleted_in_project.push(disk.clone());
            }
            State::DeletedInMirror => {
                say(report, "deleted-in-mirror");
                pending.deleted_in_mirror.push(disk.clone());
            }
            State::MirrorChanged => {
                say(report, "mirror-changed");
                pending.changed.push(disk.clone());
            }
            State::Conflict => {
                say(report, "conflict");
                pending.changed.push(disk.clone());
            }
            // What `Writer::mirror` checks before it writes, read-only.
            State::ProjectChanged if writer.plan => {
                if writer.paused || writer.blocked {
                } else if let Some(why) = crate::staging::blocked(&swap_parent(repo).0, repo.root())
                {
                    writer.blocked = true;
                    report.errors.push(why);
                } else {
                    report.changes.push((Change::Mirrored, disk.clone()));
                }
            }
            State::ProjectChanged => {
                if let Some(content) = &evaluated.disk {
                    writer.mirror(
                        root,
                        item,
                        content,
                        evaluated.mirror_hash.as_deref(),
                        synced,
                        report,
                    );
                }
            }
        }
    }
    let mut all = |what: &'static str, message: String| {
        report
            .items
            .push(note(&scope, root.dir.clone(), what, message, false));
    };
    if pending.not_here > 0 {
        all(
            "not-on-machine",
            format!(
                "{} project item(s) in the mirror are not on this machine yet; write them with: {}",
                pending.not_here,
                command("restore", std::slice::from_ref(&root.dir))
            ),
        );
    }
    if writer.plan && pending.away > 0 {
        let (id, n) = (&root.id, pending.away);
        let message = format!(
            "project root {id}: {n} item(s) are only in the mirror: their project folders are not on this machine"
        );
        all("away", message);
    }
    if pending.deleted_in_project.len() > 1 {
        all(
            "restore-all",
            format!(
                "to bring them all back: {}",
                command("restore", &pending.deleted_in_project)
            ),
        );
    }
    if pending.deleted_in_mirror.len() > 1 {
        all(
            "adopt-all",
            format!(
                "to take every item deleted in the mirror back in: {}",
                command("adopt", &pending.deleted_in_mirror)
            ),
        );
    }
    if pending.changed.len() > 1 {
        all(
            "choose-all",
            format!(
                "to keep the project's version of each changed item: {}; or the mirror's: {}",
                command("adopt", &pending.changed),
                command("restore", &pending.changed)
            ),
        );
    }
}

/// Writes project changes into the mirror (row 13).
struct Writer<'a> {
    repo: &'a Canon,
    backups: Backups,
    /// A staging for the mirror could not be opened: that is reported
    /// once, and nothing more is mirrored this run.
    blocked: bool,
    /// [`Output::Plan`]: nothing is written.
    plan: bool,
    /// A git operation in the canon, or a linked folder that cannot be read,
    /// pauses project sync; a plan still classifies.
    paused: bool,
}

impl Writer<'_> {
    fn mirror(
        &mut self,
        root: &Root,
        item: &Item,
        content: &Content,
        expected: Option<&str>,
        synced: &mut Synced,
        report: &mut SyncReport,
    ) {
        if self.blocked {
            return;
        }
        let scope = root.dir.display().to_string();
        let disk = item.disk(root);
        let (swap_dir, saved) = swap_parent(self.repo);
        let notes = &mut report.items;
        let say = |l| notes.push(note(&scope, swap_dir.clone(), "swap", l, false));
        let (repo, backups) = (self.repo.root(), &mut self.backups);
        let staging = match Staging::open(&swap_dir, "", repo, backups, saved, say) {
            Ok(staging) => staging,
            Err(e) => {
                self.blocked = true;
                return report.errors.push(e.to_string());
            }
        };
        let dest = self.repo.root().join(item.mirror_rel());
        let outcome = Replace {
            dest: &dest,
            kind: item.kind,
            expected: &Expect::from_hash(expected),
            new: content,
            backup_rel: &Path::new("repo").join(item.mirror_rel()),
        }
        .run(&staging, &mut self.backups);
        match outcome {
            Ok(Outcome::Replaced {
                previous,
                late_write,
            }) => {
                synced.set(root, &item.rel, item.kind, &content.hash());
                report.changes.push((Change::Mirrored, disk.clone()));
                match previous {
                    Previous::At(backup) if late_write => report.items.push(note(
                        &scope,
                        disk,
                        "late-write",
                        format!(
                            "a write to the mirror of {} landed while it was replaced; that version is in {}",
                            item.disk(root).display(),
                            backup.display()
                        ),
                        true,
                    )),
                    Previous::Kept(why) => report
                        .errors
                        .push(format!("{} is mirrored, but {why}", disk.display())),
                    _ => {}
                }
            }
            Ok(Outcome::Conflict(why)) => report.items.push(note(
                &scope,
                disk,
                "conflict",
                format!("{why}; run `skiman sync` again"),
                true,
            )),
            Ok(Outcome::Failed(why)) => report.errors.push(why),
            Err(e) => report
                .errors
                .push(format!("cannot write {}: {e}", dest.display())),
        }
    }
}

/// `<repo>/.skiman/swap` (mirror staging folders) and its path in a backup.
pub fn swap_parent(repo: &Canon) -> (PathBuf, &'static Path) {
    (repo.local_dir().join("swap"), "repo/.skiman/swap".as_ref())
}

fn note(
    scope: &str,
    path: PathBuf,
    what: &'static str,
    message: String,
    attention: bool,
) -> Report {
    Report {
        scope: scope.to_owned(),
        path,
        kind: ItemKind::Project {
            what,
            message,
            attention,
        },
    }
}
