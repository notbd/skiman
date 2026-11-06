//! The project part of `skiman sync` (plan "Sync: the state table"): per
//! mirror item and per synced-hash entry, never by walking the roots. Sync
//! writes only the mirror (row 13) and machine state; it never writes a
//! project folder and never spawns git.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::content::Content;
use super::git::operation_in_progress;
use super::item::{Item, MIRROR_DIR, read_mirror};
use super::root::{self, Root};
use super::state::{State, evaluate};
use super::swap::{self, Backups, Expect, Outcome, Place};
use super::synced::Synced;
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::rules::{self, Scope, Violation};
use crate::sync::{Change, Item as Report, ItemKind, Report as SyncReport};
use crate::tree::{Overlay, WorkingTree};

/// Reconciles every project root into `report`.
pub fn reconcile(paths: &Paths, repo: &SkillRepo, manifest: &Manifest, report: &mut SyncReport) {
    let scope = repo.root().join(MIRROR_DIR).display().to_string();
    if manifest.projects().is_empty() {
        return;
    }
    if let Some(operation) = operation_in_progress(repo.root()) {
        report.items.push(note(
            &scope,
            repo.root().join(MIRROR_DIR),
            "paused",
            format!(
                "the skill repo {operation}, so project items are not synced; finish that, then run `skiman sync`"
            ),
            false,
        ));
        return;
    }
    let mut synced = Synced::load(paths);
    let configured: Vec<String> = manifest.projects().into_iter().map(|p| p.id).collect();
    synced.forget_unconfigured(&configured);
    let mut backups = Backups::new(paths);
    let mut recovered = false;
    for resolved in root::resolve(paths.home(), repo, manifest) {
        match resolved {
            Err(problem) => report.items.push(note(
                &scope,
                repo.root().join(MIRROR_DIR).join(&problem.id),
                "root",
                if problem.absent {
                    format!(
                        "project root `{}` is skipped: {}",
                        problem.id, problem.message
                    )
                } else {
                    format!(
                        "project root `{}` is skipped: {}; fix [projects.{}] in skiman.toml",
                        problem.id, problem.message, problem.id
                    )
                },
                false,
            )),
            Ok(root) => reconcile_root(
                repo,
                &root,
                &mut synced,
                &mut backups,
                &mut recovered,
                report,
            ),
        }
    }
    if let Err(e) = synced.save(paths) {
        report
            .errors
            .push(format!("cannot write {}: {e}", paths.state_dir().display()));
    }
}

fn reconcile_root(
    repo: &SkillRepo,
    root: &Root,
    synced: &mut Synced,
    backups: &mut Backups,
    recovered: &mut bool,
    report: &mut SyncReport,
) {
    let scope = root.dir.display().to_string();
    let tree = WorkingTree::new(repo.root());
    let mirror = match read_mirror(&tree, root) {
        Ok(mirror) => mirror,
        Err(e) => {
            report.errors.push(format!(
                "cannot read {}/{MIRROR_DIR}/{}: {e}",
                repo.root().display(),
                root.id
            ));
            return;
        }
    };
    let mut items = mirror.items;
    let known: BTreeSet<String> = items.iter().map(|i| i.rel.clone()).collect();
    for (rel, kind) in synced.items(root) {
        if known.contains(&rel) {
            continue;
        }
        // Row 3: a recorded path that no longer has an item shape.
        match Item::at(root, &rel, kind == super::pattern::Kind::Skill) {
            Some(item) if item.kind == kind => items.push(item),
            _ => synced.remove(root, &rel),
        }
    }
    let mut gone_anchors = BTreeSet::new();
    let mut not_here = Vec::new();
    let mut to_restore = Vec::new();
    let mut to_adopt = Vec::new();
    let mut to_choose = Vec::new();
    for item in items {
        let recorded = synced.get(root, &item.rel).map(|(_, h)| h.clone());
        let evaluated = match evaluate(root, &item, repo.root(), recorded.as_deref()) {
            Ok(evaluated) => evaluated,
            Err(e) => {
                report
                    .errors
                    .push(format!("cannot read {}: {e}", item.disk(root).display()));
                continue;
            }
        };
        let disk = item.disk(root);
        let one = std::slice::from_ref(&disk);
        let attention = evaluated.state.needs_attention();
        let mut say = |what: &'static str, message: String| {
            report
                .items
                .push(note(&scope, disk.clone(), what, message, attention));
        };
        match evaluated.state {
            State::Forgotten => synced.remove(root, &item.rel),
            State::Nothing | State::NotHere | State::New => {}
            State::InSync => {
                if let Some(hash) = &evaluated.disk_hash {
                    synced.set(root, &item.rel, item.kind, hash);
                }
            }
            State::ProjectGone => {
                let anchor = item.anchor_dir(root);
                if gone_anchors.insert(anchor.clone()) {
                    say(
                        "gone",
                        format!(
                            "project folder {} is gone; its items stay in the mirror. If it moved, adopt the new path; to stop managing it: {}",
                            anchor.display(),
                            command("eject", std::slice::from_ref(&anchor))
                        ),
                    );
                }
            }
            State::NotOnMachine => not_here.push(disk.clone()),
            State::MirrorInvalid(why) => say(
                "mirror-invalid",
                format!(
                    "the mirror {} cannot be used: {why}; fix it in the skill repo (`skiman check` lists the rules)",
                    item.mirror_rel()
                ),
            ),
            State::Occupied(what) => say(
                "occupied",
                format!(
                    "project item {} is not synced: {what}; move it away, or stop managing it: {}",
                    disk.display(),
                    command("eject", one)
                ),
            ),
            State::DeletedInProject => {
                say(
                    "deleted-in-project",
                    format!(
                        "{} was deleted in the project; bring it back from the mirror: {}; or stop managing it: {}",
                        disk.display(),
                        command("restore", one),
                        command("eject", one)
                    ),
                );
                to_restore.push(disk.clone());
            }
            State::DeletedInMirror => {
                say(
                    "deleted-in-mirror",
                    format!(
                        "{} was deleted in the mirror; take it back in: {}; or stop managing it: {}",
                        item.mirror_rel(),
                        command("adopt", one),
                        command("eject", one)
                    ),
                );
                to_adopt.push(disk.clone());
            }
            State::Blocked(why) => say("blocked", blocked_message(&disk, &why)),
            State::MirrorChanged => {
                say(
                    "mirror-changed",
                    format!(
                        "the mirror of {} changed; write it to the project: {}; or keep the project's version: {}",
                        disk.display(),
                        command("restore", one),
                        command("adopt", one)
                    ),
                );
                to_choose.push(disk.clone());
            }
            State::Conflict => {
                say(
                    "conflict",
                    format!(
                        "{} and its mirror both changed; keep the project's version: {}; or the mirror's: {} (the other goes to a backup)",
                        disk.display(),
                        command("adopt", one),
                        command("restore", one)
                    ),
                );
                to_choose.push(disk.clone());
            }
            State::ProjectChanged => {
                let Some(content) = &evaluated.disk else {
                    continue;
                };
                if let Err(violations) = check_rules(repo, &item, content) {
                    let mut why: Vec<String> = violations.iter().map(ToString::to_string).collect();
                    if let Some(hint) = rules::allow_hint(&violations) {
                        why.push(hint.replace('\n', " "));
                    }
                    report.items.push(note(
                        &scope,
                        disk.clone(),
                        "blocked",
                        blocked_message(&disk, &why.join("; ")),
                        true,
                    ));
                    continue;
                }
                if !*recovered {
                    for message in swap::recover(&swap_parent(repo), Place::Mirror, backups) {
                        report
                            .items
                            .push(note(&scope, swap_parent(repo), "swap", message, false));
                    }
                    *recovered = true;
                }
                write_mirror(
                    repo,
                    root,
                    &item,
                    content,
                    evaluated.mirror_hash.as_deref(),
                    synced,
                    backups,
                    report,
                );
            }
        }
    }
    if !not_here.is_empty() {
        report.items.push(note(
            &scope,
            root.dir.clone(),
            "not-on-machine",
            format!(
                "{} project item(s) in the mirror are not on this machine yet; write them with: {}",
                not_here.len(),
                command("restore", std::slice::from_ref(&root.dir))
            ),
            false,
        ));
    }
    let mut all = |what: &'static str, message: String| {
        report
            .items
            .push(note(&scope, root.dir.clone(), what, message, false));
    };
    if to_restore.len() > 1 {
        all(
            "restore-all",
            format!(
                "to bring them all back: {}",
                command("restore", &to_restore)
            ),
        );
    }
    if to_adopt.len() > 1 {
        all(
            "adopt-all",
            format!(
                "to take every item deleted in the mirror back in: {}",
                command("adopt", &to_adopt)
            ),
        );
    }
    if to_choose.len() > 1 {
        all(
            "choose-all",
            format!(
                "to keep the project's version of each changed item: {}; or the mirror's: {}",
                command("adopt", &to_choose),
                command("restore", &to_choose)
            ),
        );
    }
}

fn blocked_message(disk: &Path, why: &str) -> String {
    format!(
        "{} is not copied into the mirror: {why}; fix it in the project, or bring back the mirror's version: {}",
        disk.display(),
        command("restore", &[disk.to_path_buf()])
    )
}

/// Row 13: copies the project's version into the mirror.
#[allow(clippy::too_many_arguments)]
fn write_mirror(
    repo: &SkillRepo,
    root: &Root,
    item: &Item,
    content: &Content,
    expected: Option<&str>,
    synced: &mut Synced,
    backups: &mut Backups,
    report: &mut SyncReport,
) {
    let scope = root.dir.display().to_string();
    let disk = item.disk(root);
    let backup_rel = Path::new("repo").join(item.mirror_rel());
    let outcome = swap::replace(
        &repo.root().join(item.mirror_rel()),
        item.kind,
        &Expect::from_hash(expected),
        content,
        &swap_parent(repo),
        Place::Mirror,
        backups,
        &backup_rel,
    );
    match outcome {
        Ok(Outcome::Replaced { backup, late_write }) => {
            synced.set(root, &item.rel, item.kind, &content.hash());
            report.changes.push((Change::Mirrored, disk.clone()));
            if late_write && let Some(backup) = backup {
                report.items.push(note(
                    &scope,
                    disk,
                    "late-write",
                    format!(
                        "a write to the mirror landed while it was replaced; that version is in {}",
                        backup.display()
                    ),
                    true,
                ));
            }
        }
        Ok(Outcome::Conflict(why)) => report.items.push(note(
            &scope,
            disk,
            "conflict",
            format!("{why}; run `skiman sync` again"),
            true,
        )),
        Ok(Outcome::Kept(why)) => report.errors.push(why),
        Err(e) => report.errors.push(format!(
            "cannot write {}: {e}",
            repo.root().join(item.mirror_rel()).display()
        )),
    }
}

/// The safety rules on `content` as it would sit at the item's mirror.
pub fn check_rules(repo: &SkillRepo, item: &Item, content: &Content) -> Result<(), Vec<Violation>> {
    let tree = WorkingTree::new(repo.root());
    let base = item.mirror_rel();
    let files: Vec<(String, Vec<u8>)> = content
        .files()
        .into_iter()
        .map(|(path, _, bytes)| {
            let full = if path.is_empty() {
                base.clone()
            } else {
                format!("{base}/{path}")
            };
            (full, bytes.to_vec())
        })
        .collect();
    let executable = content
        .files()
        .into_iter()
        .filter(|(_, x, _)| *x)
        .map(|(path, _, _)| format!("{base}/{path}"))
        .collect();
    let overlay = Overlay::new(&tree, files)
        .with_executable(executable)
        .replacing(vec![base.clone()]);
    let scope = Scope {
        projects: BTreeSet::from([base]),
        ..Scope::default()
    };
    match rules::check(&overlay, &scope) {
        Ok(violations) if violations.is_empty() => Ok(()),
        Ok(violations) => Err(violations),
        Err(e) => Err(vec![Violation::new(
            item.mirror_rel(),
            None,
            rules::rule::PROJECT_PATH,
            format!("cannot read the skill repo: {e}"),
        )]),
    }
}

/// `<repo>/.skiman/swap`: where mirror-side swap folders go.
pub fn swap_parent(repo: &SkillRepo) -> PathBuf {
    repo.local_dir().join("swap")
}

/// One paste-ready command line naming `paths`.
pub fn command(verb: &str, paths: &[PathBuf]) -> String {
    let quoted: Vec<String> = paths
        .iter()
        .map(|p| shell_quote(&p.display().to_string()))
        .collect();
    format!("skiman project {verb} {}", quoted.join(" "))
}

fn shell_quote(text: &str) -> String {
    if text
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._-~".contains(&b))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
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
