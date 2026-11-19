//! `skiman update [names]`: fetch each third-party skill's new upstream
//! version, merge it three-way with the local modifications, and swap the
//! result in (spec Merge semantics). Without names, every third-party skill.
//! Each skill is processed on its own; the exit code is the highest per-skill
//! result. Ends with a sync; never commits.
//!
//! First, without the repo lock, so that fetching (network, credentials)
//! holds up no other command, each skill is [`prepare`]d:
//!
//! 1. [`Current::read`]: the skill's records; refusals before anything is
//!    fetched (unresolved conflicts, a missing record or skill folder).
//!    A refusal is read again under the repo lock, as another command may
//!    have been writing the skill; only a refusal there stands.
//! 2. [`fetch`]: the version to move to (spec Version semantics) and its
//!    snapshot; a local folder is re-read.
//! 3. When the snapshot changes, [`merge`] in a temporary repository
//!    (conflicts are written into the result, exit 3).
//!
//! The merge base is the snapshot the lock entry records
//! ([`upstream::locked_snapshot`]):
//! `upstream/<name>` when it matches the entry, else read again from where
//! the entry says it came from.
//!
//! `--dry-run` stops there: [`plan`] checks each merge against the rules
//! and [`report`] prints what an update would do, conflicts included; it
//! writes nothing in the skill repo or the tool directories (fetching
//! fills the upstream cache). Otherwise, under the repo lock,
//! [`update_to`] refuses a skill whose records or `skills/<name>` changed
//! since, and writes the others ([`crate::change`]) in this order:
//! `skills/<name>` (when the merge changes it), `source.ref` and `ref_kind`,
//! the lock entry, then `upstream/<name>`; the old folders go to the backup.
//! Stopped at any point, the same command finishes it: until the lock entry
//! is written, a merge in place is recognised from what the stopped run
//! swapped out ([`merged_before`]), and after that the base comes from the
//! new entry.

use std::collections::BTreeSet;
use std::path::Path;

use crate::Exit;
use crate::backup::Backups;
use crate::change::{Change, Held, Want};
use crate::commands::prompts;
use crate::failure::{Failure, finish};
use crate::fetch::{Cache, RefKind, ResolvedRef};
use crate::fsutil::read_text;
use crate::git::Prompts;
use crate::layout::skill_path;
use crate::linked::Linked;
use crate::lockfile::{self, LockEntry, Lockfile};
use crate::manifest::{self, Manifest, SourceRecord};
use crate::merge::{Conflict, ConflictKind, MergeRepo, Outcome};
use crate::path_safety::{self, Content};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, CONFLICT_MARKERS, FRONTMATTER_RULES, Scope, Violation, rule};
use crate::skill_md::SkillMd;
use crate::snapshot::Snapshot;
use crate::source::{UrlKind, check_url};
use crate::staging;
use crate::sync::{self, Output};
use crate::test_hook;
use crate::tree::WorkingTree;
use crate::upstream::{self, Upstream, print_not_installed, run_flags};

pub fn run(names: Vec<String>, git_ref: Option<String>, dry_run: bool) -> Exit {
    finish(update(names, git_ref, dry_run))
}

/// What every skill's write shares, under the repo lock.
struct Context<'a> {
    home: &'a Path,
    repo: &'a SkillRepo,
    lock: RepoLock,
    /// One backup folder for the command: tmp leftovers and old folders.
    backups: Backups,
}

fn update(names: Vec<String>, git_ref: Option<String>, dry_run: bool) -> Result<Exit, Failure> {
    if git_ref.is_some() && names.len() != 1 {
        return Err(Failure::usage("--ref needs exactly one skill name"));
    }
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let prompts = prompts();
    let names = if names.is_empty() {
        let manifest = Manifest::load(&repo)?;
        let names: Vec<String> = manifest
            .skills()
            .into_keys()
            .filter(|name| manifest.source(name).is_some())
            .collect();
        if names.is_empty() {
            out!("no third-party skills to update");
        }
        names
    } else {
        let mut seen = BTreeSet::new();
        names
            .into_iter()
            .filter(|name| seen.insert(name.clone()))
            .collect()
    };

    let mut exit = Exit::Success;
    let mut prepared = Vec::new();
    for name in &names {
        match prepare(&paths, &repo, prompts, name, git_ref.as_deref(), dry_run) {
            Ok(skill) => prepared.push(skill),
            Err(failure) => {
                err!("error: skill {name}: {}", failure.message);
                exit = exit.combine(failure.exit);
            }
        }
    }
    // A dry run, or a run with nothing to write, takes no lock to write.
    let mut context = if dry_run || (prepared.is_empty() && !names.is_empty()) {
        None
    } else {
        Some(Context {
            home: paths.home(),
            repo: &repo,
            lock: RepoLock::wait(&paths, &repo)?,
            backups: Backups::new(&paths),
        })
    };
    let mut any_done = names.is_empty();
    for skill in &prepared {
        let result = match context.as_mut() {
            Some(context) => update_to(context, skill),
            None => plan(&repo, skill, &skill.current).and_then(|plan| {
                report(skill, &plan, plan.change.in_place(repo.root())?, true);
                let conflicts = skill.merge.as_ref().map(|m| m.conflicts.as_slice());
                print_conflicts(skill, conflicts.unwrap_or_default(), true);
                Ok(Exit::Success)
            }),
        };
        match result {
            Ok(result) => {
                for note in &skill.target.notes {
                    out!("  note: {note}");
                }
                any_done = true;
                exit = exit.combine(result);
            }
            Err(failure) => {
                err!("error: skill {}: {}", skill.name, failure.message);
                exit = exit.combine(failure.exit);
            }
        }
    }
    // The warnings once for the command, on the skill repo as it leaves it,
    // excusing the conflicts each merge wrote.
    let excuses: Vec<_> = prepared
        .iter()
        .filter_map(|s| s.merge.as_ref().map(|m| excused(&s.current.skill, m)))
        .collect();
    if !excuses.is_empty() {
        let tree = WorkingTree::new(repo.root());
        if let Err(failure) = rules::warn(&tree, &|v| excuses.iter().any(|e| e(v))) {
            exit = exit.combine(failure.report());
        }
    }
    match context {
        Some(context) if any_done => {
            exit = exit.combine(sync::run(&paths, &repo, &context.lock, Output::Normal));
        }
        None if dry_run => out!("dry run: nothing was written"),
        _ => {}
    }
    Ok(exit)
}

/// One skill's records as they are before its update.
struct Current {
    /// The skill folder, `skills/[<group>/]<name>`.
    skill: String,
    source: SourceRecord,
    entry: LockEntry,
    manifest_text: String,
    lock_text: String,
}

impl Current {
    /// Reads the skill's records; refuses (exit 1) a skill that cannot be
    /// updated, before anything is fetched.
    fn read(home: &Path, repo: &SkillRepo, name: &str) -> Result<Current, Failure> {
        let manifest_text = read_text(&repo.manifest_file())?.unwrap_or_default();
        let manifest = Manifest::parse(&manifest_text)
            .map_err(|e| Failure::refused(format!("{}: {e}", repo.manifest_file().display())))?;
        let skill = repo.layout()?.path(name);
        let skill_dir = repo.root().join(&skill);
        let Some(source) = manifest.source(name) else {
            return Err(Failure::refused(if skill_dir.exists() {
                format!(
                    "{skill} has no [skills.{name}.source] table in skiman.toml, so it is an own skill; only third-party skills are updated"
                )
            } else if let Some((why, _)) =
                Linked::read(home, repo, &manifest).edited_elsewhere(name)
            {
                why
            } else {
                format!("there is no skill named {name}")
            }));
        };
        if source.url.is_empty() {
            return Err(Failure::refused(format!(
                "[skills.{name}.source] in skiman.toml has no `url`; run `skiman check`"
            )));
        }
        if let Err(why) = check_url(&source.url) {
            return Err(Failure::refused(format!(
                "[skills.{name}.source] in skiman.toml: the `url` `{}` {why}; nothing was fetched",
                source.url
            )));
        }
        let entry = Lockfile::load(repo)?
            .entry(name)
            .filter(|entry| !entry.snapshot.is_empty());
        let Some(entry) = entry else {
            return Err(Failure::refused(format!(
                "{name} has no entry in skiman.lock; run `skiman check`"
            )));
        };
        if !skill_dir.is_dir() || skill_dir.is_symlink() {
            return Err(Failure::refused(format!(
                "{skill} is not a folder; run `skiman check`"
            )));
        }
        refuse_unresolved(repo, &skill, name)?;
        let lock_text = read_text(&repo.lockfile())?.unwrap_or_default();
        Ok(Current {
            skill,
            source,
            entry,
            manifest_text,
            lock_text,
        })
    }
}

/// Refuses (exit 1) a skill with unresolved conflicts, printing the
/// `skiman check` lines.
fn refuse_unresolved(repo: &SkillRepo, skill: &str, name: &str) -> Result<(), Failure> {
    let tree = WorkingTree::new(repo.root());
    let conflicts = rules::unresolved_conflicts(&tree, name)
        .map_err(|e| Failure::operational(format!("cannot read the skill repo: {e}")))?;
    if conflicts.is_empty() {
        return Ok(());
    }
    let lines: String = conflicts.iter().map(|v| format!("\n  {v}")).collect();
    Err(Failure::refused(format!(
        "{skill} has unresolved conflicts; resolve them before updating:{lines}"
    )))
}

/// The version an update moves to.
struct Target {
    snapshot: Snapshot,
    /// The new lock entry.
    entry: LockEntry,
    /// The ref it was read at; `None` for the default branch or a local folder.
    resolved: Option<ResolvedRef>,
    /// `  note: ...` lines printed after the skill's result.
    notes: Vec<String>,
}

/// One skill as [`prepare`] read it: its records, the version to move to
/// and, when the snapshot changes, the merge.
struct Prepared {
    name: String,
    current: Current,
    target: Target,
    merge: Option<Merge>,
}

/// A merge, with `skills/<name>` as merged.
struct Merge {
    ours: Snapshot,
    merged: Snapshot,
    conflicts: Vec<Conflict>,
}

/// Reads the skill's records, fetches its new version and merges it;
/// writes nothing.
fn prepare(
    paths: &Paths,
    repo: &SkillRepo,
    prompts: Prompts,
    name: &str,
    git_ref: Option<&str>,
    dry_run: bool,
) -> Result<Prepared, Failure> {
    // Read without the lock: another command may be writing the skill.
    // Under the lock the records are whole: a refusal then is real, and
    // records read there are checked again by `update_to` like any others.
    let current = Current::read(paths.home(), repo, name).or_else(|_| {
        test_hook::at(name, "read-again");
        let _lock = RepoLock::wait(paths, repo)?;
        // A replace stopped between its two renames left the path empty and
        // the new one in tmp; a dry run writes nothing, so it leaves it there.
        // Only a skill that is in no folder now is looked for, and put back
        // once: a leftover for a folder the skill has since left must not
        // make a second copy. The empty path is in no group or in a group
        // folder still there; a write command that empties the group moves
        // the leftover to its backup first.
        let layout = repo.layout()?;
        if !dry_run && layout.find(name).is_none() {
            let groups = layout.groups().iter().map(|g| Some(g.as_str()));
            let skills: Vec<String> = std::iter::once(None)
                .chain(groups)
                .map(|group| skill_path(group, name))
                .collect();
            let tmp = staging::list(&repo.tmp_dir()).unwrap_or_default();
            let put = tmp.iter().find_map(|t| {
                skills
                    .iter()
                    .find_map(|skill| staging::finish_swap(t, repo.root(), skill))
            });
            if let Some(line) = put {
                out!("{line}");
            }
        }
        Current::read(paths.home(), repo, name)
    })?;
    let mut target = fetch(paths, prompts, name, &current, git_ref)?;
    // A snapshot that does not match the lock is replaced on either path.
    if Snapshot::from_folder(&repo.upstream_dir(name))?.tree_hash() != current.entry.snapshot {
        let is = if dry_run { "would be" } else { "is" };
        target.notes.push(format!(
            "upstream/{name} did not match skiman.lock; it {is} replaced by the snapshot the lock records, and what it held {is} moved to the backup"
        ));
    }
    let merge = if target.entry.snapshot == current.entry.snapshot {
        None
    } else {
        let ours = Snapshot::from_folder(&repo.root().join(&current.skill))?;
        let (source, entry) = (&current.source, &current.entry);
        let locked = upstream::locked_snapshot(paths, prompts, repo, name, source, entry)?;
        let base = locked.ok_or_else(|| {
            Failure::refused(format!(
                "upstream/{name} does not match skiman.lock, and neither does {} at the locked version; restore upstream/{name} before updating",
                source.url
            ))
        })?;
        let snapshots = [&base, &ours, &target.snapshot];
        let outcome = match merged_before(paths, repo, &current.skill, name, snapshots) {
            true => Outcome::Clean(ours.clone()),
            false => merge(&base, &ours, &target.snapshot)?,
        };
        let (merged, conflicts) = match outcome {
            Outcome::Clean(merged) => (merged, Vec::new()),
            Outcome::Conflicts { merged, conflicts } => (merged, conflicts),
        };
        Some(Merge {
            merged: with_install_name(name, merged),
            ours,
            conflicts,
        })
    };
    test_hook::at(name, "prepared");
    Ok(Prepared {
        name: name.to_owned(),
        current,
        target,
        merge,
    })
}

/// Whether `ours` already holds this merge, as an update stopped before the
/// lock leaves it: an old skill folder `skill` (in a tmp leftover, here or
/// moved to a backup, or, to merge less, only the newest backup with it and
/// no `upstream/<name>`) merges into `ours`.
/// Merging `ours` again could apply upstream's changes twice.
fn merged_before(
    paths: &Paths,
    repo: &SkillRepo,
    skill: &str,
    name: &str,
    snapshots: [&Snapshot; 3],
) -> bool {
    let [base, ours, theirs] = snapshots;
    let skill = Path::new(skill);
    let backups = staging::list(&paths.state_dir().join("backups")).unwrap_or_default();
    let repos = backups.iter().map(|b| b.join("repo"));
    let backup = repos.clone().rfind(|b| b.join(skill).is_dir());
    let backup = backup.filter(|b| !b.join("upstream").join(name).exists());
    // tmp leftovers, and those a later write command moved to its backup.
    let moved = repos.flat_map(|b| staging::list(&b.join(".skiman/tmp")).unwrap_or_default());
    let tmp = staging::list(&repo.tmp_dir()).unwrap_or_default();
    // The swap's own fallback leaves `<name>.skiman-prev`; earlier versions left `old/prev`.
    let prev = format!("{name}{}", staging::PREV);
    let olds = tmp.into_iter().chain(moved).flat_map(|t| {
        let (old, earlier) = (t.join("old").join(skill), t.join("old/prev"));
        [old.clone(), old.with_file_name(&prev), earlier.join(skill)]
    });
    olds.chain(backup.map(|b| b.join(skill))).filter(|old| old.is_dir()).any(|old| {
        let again = Snapshot::from_folder(&old).and_then(|old| merge(base, &old, theirs));
        matches!(again, Ok(Outcome::Clean(m)) if with_install_name(name, m.clone()).files == ours.files)
    })
}

/// The refusal (exit 1) of a skill whose records or folders changed after
/// [`prepare`] read them.
fn changed(skill: &str, name: &str) -> Failure {
    Failure::refused(format!(
        "{skill}, upstream/{name} or their records changed after the update read them; nothing of {name} changed, run the update again"
    ))
}

/// What moving a skill to its target writes, from the records `current`:
/// the files the merge changes, and the [`Change`]: the merged
/// `skills/<name>`, `skiman.toml` (when `source.ref` or `ref_kind` change),
/// `skiman.lock`, then `upstream/<name>`. A merge has passed [`validate`].
struct Plan {
    changes: Vec<(FileChange, String)>,
    change: Change,
}

fn plan(repo: &SkillRepo, skill: &Prepared, current: &Current) -> Result<Plan, Failure> {
    let Prepared {
        name,
        target,
        merge,
        ..
    } = skill;
    let mut pieces = Vec::new();
    if let Some(m) = merge {
        pieces.push((current.skill.clone(), Want::Folder(m.merged.files.clone())));
    }
    if let Some(text) = new_manifest_text(name, current, target.resolved.as_ref())? {
        pieces.push((rules::MANIFEST.to_owned(), Want::File(text.into_bytes())));
    }
    let lock = lockfile::with_entry(&current.lock_text, name, &target.entry)
        .map_err(|e| Failure::refused(format!("skiman.lock: {e}")))?;
    pieces.push((rules::LOCKFILE.to_owned(), Want::File(lock.into_bytes())));
    let upstream = Want::Folder(target.snapshot.files.clone());
    pieces.push((format!("upstream/{name}"), upstream));
    let plan = Plan {
        changes: merge
            .as_ref()
            .map_or_else(Vec::new, |m| changes(&m.ours, &m.merged)),
        change: Change {
            pieces,
            command: "update",
            save_files: false,
        },
    };
    if let Some(m) = merge {
        validate(repo, &current.skill, name, m, &plan)?;
    }
    Ok(plan)
}

/// The result line of a skill (`dry_run`: what an update would do) and the
/// files it changes, flagging what agents may run.
fn report(skill: &Prepared, plan: &Plan, up_to_date: bool, dry_run: bool) {
    let (name, entry) = (&skill.name, &skill.target.entry);
    // The commit, or for a local folder the snapshot's tree hash.
    let version = entry.commit.as_ref().unwrap_or(&entry.snapshot);
    // A merge that changes no file (run again after it was in place) is none.
    match skill.merge.as_ref().filter(|_| !plan.changes.is_empty()) {
        _ if up_to_date => out!("skill {name}: up to date at {version}"),
        None if dry_run => {
            out!("skill {name}: would move to {version}; no file of the skill changes")
        }
        None => out!("skill {name}: now at {version}; no file of the skill changed"),
        Some(merge) => {
            let done = if dry_run { "would update" } else { "updated" };
            let conflicts = if merge.conflicts.is_empty() {
                ""
            } else {
                " with conflicts"
            };
            out!("skill {name}: {done} to {version}{conflicts}; changed files:");
            for (change, path) in &plan.changes {
                let flags = merge.merged.file(path).map_or_else(String::new, |file| {
                    run_flags(path, &file.content, file.executable)
                });
                out!("  {} {path}{flags}", change.verb());
            }
        }
    }
    print_not_installed(&skill.target.snapshot, "  ");
}

/// Under the repo lock: refuses the skill (exit 1) when its records or
/// `skills/<name>` changed since [`prepare`] read them, then writes what
/// its [`Plan`] does not find in place. `NeedsAttention` when conflicts
/// were written.
fn update_to(context: &mut Context, skill: &Prepared) -> Result<Exit, Failure> {
    let Prepared { name, merge, .. } = skill;
    let repo = context.repo;
    let current = Current::read(context.home, repo, name)?;
    let hash = |dir: &Path| Snapshot::from_folder(dir).map(|folder| folder.tree_hash());
    let unchanged = (&current.source, &current.entry)
        == (&skill.current.source, &skill.current.entry)
        && current.skill == skill.current.skill
        && match merge {
            Some(m) => hash(&repo.root().join(&current.skill))? == m.ours.tree_hash(),
            None => true,
        };
    if !unchanged {
        return Err(changed(&current.skill, name));
    }
    let plan = plan(repo, skill, &current)?;
    let held = plan.change.held(repo.root())?;
    if held.iter().all(|h| *h == Held::Same) {
        report(skill, &plan, true, false);
        return Ok(Exit::Success);
    }
    plan.change
        .write(&held, repo, &context.lock, &mut context.backups)?;
    report(skill, &plan, false, false);
    if let (Some(dir), true) = (context.backups.created(), plan.change.backs_up(&held)) {
        out!("saved the old folders of {name} to {}", dir.display());
    }
    let conflicts = merge.as_ref().map(|m| m.conflicts.as_slice());
    print_conflicts(skill, conflicts.unwrap_or_default(), false);
    Ok(match conflicts.is_some_and(|c| !c.is_empty()) {
        true => Exit::NeedsAttention,
        false => Exit::Success,
    })
}

/// The conflicts a merge wrote into the skill folder (`dry_run`: would write).
fn print_conflicts(skill: &Prepared, conflicts: &[Conflict], dry_run: bool) {
    if conflicts.is_empty() {
        return;
    }
    let (name, folder) = (&skill.name, &skill.current.skill);
    match dry_run {
        true => out!("skill {name}: the update would write these conflicts into {folder}:"),
        false => out!(
            "skill {name}: resolve these conflicts in {folder}, then delete the conflict records:"
        ),
    }
    for conflict in conflicts {
        out!("  {}", describe(conflict));
    }
}

/// One line of the conflict list, naming the file and its record.
fn describe(conflict: &Conflict) -> String {
    let path = &conflict.path;
    match &conflict.kind {
        ConflictKind::Markers => format!("{path}: conflict markers"),
        ConflictKind::UpstreamVersion { record } => {
            format!("{path}: upstream's version is in {record}")
        }
        ConflictKind::DeletedUpstream { record } => {
            format!("{path}: deleted upstream; {record} marks it")
        }
    }
}

/// Picks the version to move to (spec Version semantics), fetches it and
/// reads the skill's folder. The ref is `--ref`, else `source.ref`; without
/// one, the default branch. A branch moves to its head. A tag or commit
/// stays at the locked commit unless the ref changes (`--ref`, or
/// `source.ref` differing from the lock's `ref`); a moved tag is reported.
/// A local folder is re-read as it is now and versioned by its tree hash;
/// it has no refs, and a missing one is an error (exit 4) that keeps the
/// current content. `ref_kind` is always derived, never read.
fn fetch(
    paths: &Paths,
    prompts: Prompts,
    name: &str,
    current: &Current,
    requested: Option<&str>,
) -> Result<Target, Failure> {
    let source = &current.source;
    let mut notes = Vec::new();
    let upstream = if check_url(&source.url) == Ok(UrlKind::Local) {
        let unpinned = format!("remove `ref` from [skills.{name}.source] in skiman.toml");
        if let Some(remedy) = requested
            .map(|_| "drop --ref")
            .or(source.git_ref.as_ref().map(|_| unpinned.as_str()))
        {
            return Err(Failure::refused(format!(
                "{} is a local folder, which has no versions; {remedy}",
                source.url
            )));
        }
        let dir = upstream::local_root(paths, &source.url)?;
        let folder = upstream::join(&dir, &source.path);
        if !folder.is_dir() {
            return Err(Failure::operational(format!(
                "the local folder {} does not exist; nothing of {name} changed",
                folder.display()
            )));
        }
        Upstream::Local { dir }
    } else {
        let cache = Cache::open(paths, &source.url, prompts)?;
        match requested.or(source.git_ref.as_deref()) {
            None => Upstream::at_ref(cache, None)?,
            Some(git_ref) => {
                let resolved = cache.resolve_ref(git_ref)?;
                let changing = requested.is_some() || source.git_ref != current.entry.git_ref;
                let commit = pick_commit(name, &resolved, current, changing, &mut notes);
                Upstream::at(cache, commit, Some(resolved), None)?
            }
        }
    };
    let snapshot = upstream.snapshot(&source.path)?;
    let entry = upstream.lock_entry(&snapshot);
    Ok(Target {
        resolved: upstream.resolved().cloned(),
        snapshot,
        entry,
        notes,
    })
}

/// The commit a resolved ref moves the skill to.
fn pick_commit(
    name: &str,
    resolved: &ResolvedRef,
    current: &Current,
    changing: bool,
    notes: &mut Vec<String>,
) -> String {
    if changing && resolved.also_tag {
        notes.push(format!(
            "`{}` is also a tag; the branch is followed",
            resolved.name
        ));
    }
    let locked = current.entry.commit.as_ref();
    match (resolved.kind, locked) {
        (RefKind::Tag | RefKind::Commit, Some(locked)) if !changing => {
            if resolved.kind == RefKind::Tag && *locked != resolved.commit {
                notes.push(format!(
                    "tag `{}` now points to {}; {name} stays at its locked commit, and `skiman update {name} --ref {}` moves it there",
                    resolved.name,
                    resolved.commit,
                    resolved.name
                ));
            }
            locked.clone()
        }
        _ => resolved.commit.clone(),
    }
}

/// `skiman.toml` with the skill's `source.ref` and `ref_kind` derived from
/// `resolved` (both removed without one); `None` when nothing changes.
fn new_manifest_text(
    name: &str,
    current: &Current,
    resolved: Option<&ResolvedRef>,
) -> Result<Option<String>, Failure> {
    let text = manifest::with_source_ref(
        &current.manifest_text,
        name,
        resolved.map(|r| r.name.as_str()),
        resolved.map(|r| r.kind.as_str()),
    )
    .map_err(|e| Failure::refused(format!("skiman.toml: {e}")))?;
    Ok((text != current.manifest_text).then_some(text))
}

/// `merged` with its `SKILL.md` `name` line set back to the install name
/// when the merge took another one from upstream (the same edit as `add`'s;
/// a local modification). A `SKILL.md` that cannot be read is left to
/// [`validate`].
fn with_install_name(name: &str, mut merged: Snapshot) -> Snapshot {
    if let Some(file) = merged.files.iter_mut().find(|f| f.path == "SKILL.md")
        && let Ok(skill_md) = SkillMd::parse_for_rename("SKILL.md", &file.content)
        && skill_md.name() != Some(name)
        && let Ok(text) = skill_md.with_name(name)
    {
        file.content = text.into_bytes();
    }
    merged
}

/// base = `upstream/<name>`, ours = `skills/<name>` (working tree,
/// including uncommitted edits), theirs = the new snapshot.
fn merge(base: &Snapshot, ours: &Snapshot, theirs: &Snapshot) -> Result<Outcome, Failure> {
    let repo = MergeRepo::create()?;
    let base = repo.commit("base", &base.files)?;
    let ours = repo.commit("ours", &ours.files)?;
    let theirs = repo.commit("theirs", &theirs.files)?;
    repo.merge(&base, &ours, &theirs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FileChange {
    Added,
    Deleted,
    Modified,
}

impl FileChange {
    fn verb(self) -> &'static str {
        match self {
            FileChange::Added => "added",
            FileChange::Deleted => "deleted",
            FileChange::Modified => "modified",
        }
    }
}

/// The files of the skill the merge changed (content or executable bit),
/// sorted by path.
fn changes(ours: &Snapshot, merged: &Snapshot) -> Vec<(FileChange, String)> {
    let mut out = Vec::new();
    for file in &merged.files {
        match ours.file(&file.path) {
            None => out.push((FileChange::Added, file.path.clone())),
            Some(old) if old != file => out.push((FileChange::Modified, file.path.clone())),
            Some(_) => {}
        }
    }
    for file in &ours.files {
        if merged.file(&file.path).is_none() {
            out.push((FileChange::Deleted, file.path.clone()));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// The checks on the candidate output before anything is staged: path
/// safety on the merged skill folder; the files the merge created or changed
/// get every rule; the merged skill must hold a `SKILL.md`; every file of
/// the merged skill and of the new snapshot gets the secret, symlink and
/// size rules; the lock entry and Consistency for the skill. Violations
/// elsewhere, including the other rules on files the merge did not change,
/// are warnings, printed once when every skill is done. The conflict
/// representation is excused ([`excused`]).
fn validate(
    repo: &SkillRepo,
    skill: &str,
    name: &str,
    merge: &Merge,
    plan: &Plan,
) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(repo.root());
    let candidate = plan.change.overlay(&working_tree);
    path_safety::check(&candidate, skill, "the merge result", Content::Output)?;
    let names = BTreeSet::from([name.to_owned()]);
    let scope = Scope {
        files: plan
            .changes
            .iter()
            .filter(|(change, _)| *change != FileChange::Deleted)
            .map(|(_, path)| format!("{skill}/{path}"))
            .collect(),
        content_only_skills: names.clone(),
        upstreams: names.clone(),
        lock_entries: names.clone(),
        consistency: names,
        ..Scope::default()
    };
    rules::refuse_write(&candidate, &scope, &excused(skill, merge))
}

/// Whether `v` is part of the conflict representation the merge wrote into
/// the skill folder `skill`, which the rules excuse: an unresolved-conflict violation
/// in the skill, a frontmatter rule on a `SKILL.md` whose markers start
/// inside its frontmatter, a missing `SKILL.md` whose upstream version is in
/// the conflict record.
fn excused(skill: &str, merge: &Merge) -> impl Fn(&Violation) -> bool {
    let conflict = |kind: fn(&ConflictKind) -> bool| {
        let mut on_skill_md = merge.conflicts.iter().filter(|c| c.path == "SKILL.md");
        on_skill_md.any(|c| kind(&c.kind))
    };
    let markers = conflict(|k| *k == ConflictKind::Markers)
        && merge
            .merged
            .file("SKILL.md")
            .is_some_and(|f| markers_in_frontmatter(&f.content));
    let upstream_version = conflict(|k| matches!(k, ConflictKind::UpstreamVersion { .. }));
    let skill = skill.to_owned();
    move |v: &Violation| {
        (upstream_version && v.rule == rule::SKILL_MD && v.path == skill)
            || (v.path.starts_with(&format!("{skill}/"))
                && (v.rule == rule::UNRESOLVED_CONFLICT
                    || (markers
                        && v.path == format!("{skill}/SKILL.md")
                        && FRONTMATTER_RULES.contains(&v.rule))))
    }
}

/// `true` when the first conflict marker line of `content` comes before the
/// line that closes the frontmatter (or there is no closing line), so the
/// markers are what makes the frontmatter invalid.
fn markers_in_frontmatter(content: &[u8]) -> bool {
    let is_delimiter = |line: &[u8]| line.strip_suffix(b"\r").unwrap_or(line) == b"---";
    let lines: Vec<&[u8]> = content.split(|b| *b == b'\n').collect();
    let Some(first_marker) = lines.iter().position(|line| {
        CONFLICT_MARKERS
            .iter()
            .any(|m| line.starts_with(m.as_bytes()))
    }) else {
        return false;
    };
    let end = if lines.first().is_some_and(|l| is_delimiter(l)) {
        lines
            .iter()
            .skip(1)
            .position(|l| is_delimiter(l))
            .map_or(usize::MAX, |i| i + 1)
    } else {
        0
    };
    first_marker <= end
}
