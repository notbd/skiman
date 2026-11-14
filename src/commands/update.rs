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
//!    fetched (unresolved conflicts, a snapshot that differs from the lock).
//!    A refusal is read again under the repo lock, as another command may
//!    have been writing the skill; only a refusal there stands.
//! 2. [`fetch`]: the version to move to (spec Version semantics) and its
//!    snapshot; a local folder is re-read.
//! 3. When the snapshot changes, [`merge`] in a temporary repository
//!    (conflicts are written into the result, exit 3).
//!
//! `--dry-run` stops there: [`plan`] checks each merge against the rules
//! and [`report`] prints what an update would do, conflicts included; it
//! writes nothing in the skill repo or the tool directories (fetching
//! fills the upstream cache). Otherwise, under the repo lock,
//! [`update_to`] refuses a skill whose records, `skills/<name>` or
//! `upstream/<name>` changed since, and moves the others:
//!
//! 4. Same snapshot: up to date, or [`advance_records`] (fragments to a
//!    fresh backup, then `source.ref` and `ref_kind`, then the lock entry).
//! 5. Otherwise [`plan`] ([`validate`] the merge: path safety, rules),
//!    stage it in `tmp/<timestamp>/new/`, then [`swap`]: re-hash, fragments,
//!    re-hash, renames through `tmp/<timestamp>/old/`, `source.ref` and
//!    `ref_kind`, lock entry (last write), old folders to the backup; a
//!    failure before the lock entry prints the recovery ([`stopped`]).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup::Backups;
use crate::commands::prompts;
use crate::failure::{Failure, finish};
use crate::fetch::{Cache, RefKind, ResolvedRef};
use crate::fsutil::{self, read_text, write_atomic};
use crate::git::Prompts;
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
use crate::staging::Staging;
use crate::sync::{self, Output};
use crate::test_hook;
use crate::tree::{Overlay, WorkingTree, tree_hash_of_dir};
use crate::upstream::{self, Upstream, print_not_installed, run_flags};

pub fn run(names: Vec<String>, git_ref: Option<String>, dry_run: bool) -> Exit {
    finish(update(names, git_ref, dry_run))
}

/// What every skill's write shares, under the repo lock.
struct Context<'a> {
    paths: &'a Paths,
    repo: &'a SkillRepo,
    lock: RepoLock,
    /// The command's `tmp/<timestamp>/`, begun before the first skill writes
    /// (a merge or a records-only advance), so tmp leftovers move first.
    staging: Option<Staging>,
}

impl Context<'_> {
    fn staging(&mut self) -> Result<&Staging, Failure> {
        if self.staging.is_none() {
            self.staging = Some(Staging::begin(self.paths, self.repo, &self.lock)?);
        }
        Ok(self.staging.as_ref().expect("just begun"))
    }
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
        match prepare(&paths, &repo, prompts, name, git_ref.as_deref()) {
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
            paths: &paths,
            repo: &repo,
            lock: RepoLock::wait(&paths, &repo)?,
            staging: None,
        })
    };
    let mut any_done = names.is_empty();
    for skill in &prepared {
        let result = match context.as_mut() {
            Some(context) => update_to(context, skill),
            None => plan(&repo, skill, &skill.current).map(|plan| {
                report(skill, &plan, true);
                let conflicts = skill.merge.as_ref().map(|m| m.conflicts.as_slice());
                print_conflicts(&skill.name, conflicts.unwrap_or_default(), true);
                Exit::Success
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
    match context {
        Some(context) if any_done => {
            drop(context.staging);
            exit = exit.combine(sync::run(&paths, &repo, &context.lock, Output::Normal));
        }
        None if dry_run => out!("dry run: nothing was written"),
        _ => {}
    }
    Ok(exit)
}

/// One skill's records as they are before its update.
struct Current {
    source: SourceRecord,
    entry: LockEntry,
    manifest_text: String,
    lock_text: String,
}

impl Current {
    /// Reads the skill's records; refuses (exit 1) a skill that cannot be
    /// updated, before anything is fetched.
    fn read(repo: &SkillRepo, name: &str) -> Result<Current, Failure> {
        let manifest_text = read_text(&repo.manifest_file())?.unwrap_or_default();
        let manifest = Manifest::parse(&manifest_text)
            .map_err(|e| Failure::refused(format!("{}: {e}", repo.manifest_file().display())))?;
        let skill_dir = repo.skill_dir(name);
        let Some(source) = manifest.source(name) else {
            return Err(Failure::refused(if skill_dir.exists() {
                format!(
                    "skills/{name} has no [skills.{name}.source] table in skiman.toml, so it is an own skill; only third-party skills are updated"
                )
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
                "skills/{name} is not a folder; run `skiman check`"
            )));
        }
        refuse_unresolved(repo, name)?;
        let lock_text = read_text(&repo.lockfile())?.unwrap_or_default();
        Ok(Current {
            source,
            entry,
            manifest_text,
            lock_text,
        })
    }
}

/// Refuses (exit 1) a skill with unresolved conflicts, and one whose
/// `upstream/<name>` does not match its lock entry, printing the
/// `skiman check` lines.
fn refuse_unresolved(repo: &SkillRepo, name: &str) -> Result<(), Failure> {
    let io = |e: std::io::Error| Failure::operational(format!("cannot read the skill repo: {e}"));
    let tree = WorkingTree::new(repo.root());
    let lines = |violations: Vec<Violation>| -> String {
        violations.iter().map(|v| format!("\n  {v}")).collect()
    };
    let conflicts = rules::unresolved_conflicts(&tree, name).map_err(io)?;
    if !conflicts.is_empty() {
        return Err(Failure::refused(format!(
            "skills/{name} has unresolved conflicts; resolve them before updating:{}",
            lines(conflicts)
        )));
    }
    let scope = Scope {
        consistency: BTreeSet::from([name.to_owned()]),
        ..Scope::default()
    };
    let snapshot = rules::check(&tree, &scope).map_err(io)?;
    if !snapshot.is_empty() {
        return Err(Failure::refused(format!(
            "upstream/{name} does not match skiman.lock; restore the snapshot before updating:{}",
            lines(snapshot)
        )));
    }
    Ok(())
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
) -> Result<Prepared, Failure> {
    // Read without the lock: another command may be writing the skill.
    // Under the lock the records are whole: a refusal then is real, and
    // records read there are checked again by `update_to` like any others.
    let current = Current::read(repo, name).or_else(|_| {
        test_hook::at(name, "read-again");
        let _lock = RepoLock::wait(paths, repo)?;
        Current::read(repo, name)
    })?;
    let target = fetch(paths, prompts, name, &current, git_ref)?;
    let merge = if target.entry.snapshot == current.entry.snapshot {
        None
    } else {
        let ours = Snapshot::from_folder(&repo.skill_dir(name))?;
        let base = Snapshot::from_folder(&repo.upstream_dir(name))?;
        // `Current::read` found `upstream/<name>` matching the lock entry;
        // under the lock it does again, and the entry must be the same.
        if base.tree_hash() != current.entry.snapshot {
            return Err(changed(name));
        }
        let (merged, conflicts) = match merge(&base, &ours, &target.snapshot)? {
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

/// The refusal (exit 1) of a skill whose records or folders changed after
/// [`prepare`] read them.
fn changed(name: &str) -> Failure {
    Failure::refused(format!(
        "skills/{name}, upstream/{name} or their records changed after the update read them; nothing of {name} changed, run the update again"
    ))
}

/// What moving a skill to its target writes, from the records `current`:
/// the files the merge changes, and the new `skiman.toml` (`None` when it
/// stays the same) and `skiman.lock`. A merge has passed [`validate`].
struct Plan {
    changes: Vec<(Change, String)>,
    manifest: Option<String>,
    lock: String,
}

fn plan(repo: &SkillRepo, skill: &Prepared, current: &Current) -> Result<Plan, Failure> {
    let Prepared {
        name,
        target,
        merge,
        ..
    } = skill;
    let plan = Plan {
        changes: merge
            .as_ref()
            .map_or_else(Vec::new, |m| changes(&m.ours, &m.merged)),
        manifest: new_manifest_text(name, current, target.resolved.as_ref())?,
        lock: lockfile::with_entry(&current.lock_text, name, &target.entry)
            .map_err(|e| Failure::refused(format!("skiman.lock: {e}")))?,
    };
    if let Some(m) = merge {
        validate(repo, name, &m.merged, &target.snapshot, &plan, &m.conflicts)?;
    }
    Ok(plan)
}

/// Whether the skill stays as it is: same snapshot, lock entry and records.
fn up_to_date(skill: &Prepared, plan: &Plan) -> bool {
    skill.merge.is_none() && skill.target.entry == skill.current.entry && plan.manifest.is_none()
}

/// The result line of a skill (`dry_run`: what an update would do) and the
/// files it changes, flagging what agents may run.
fn report(skill: &Prepared, plan: &Plan, dry_run: bool) {
    let (name, entry) = (&skill.name, &skill.target.entry);
    // The commit, or for a local folder the snapshot's tree hash.
    let version = entry.commit.as_ref().unwrap_or(&entry.snapshot);
    match &skill.merge {
        _ if up_to_date(skill, plan) => out!("skill {name}: up to date at {version}"),
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
/// `skills/<name>` changed since [`prepare`] read them (`upstream/<name>`
/// is checked by `Current::read` against the same lock entry), then moves
/// it to its target: records only when its snapshot is the same, else
/// validate and swap. `NeedsAttention` when conflicts were written.
fn update_to(context: &mut Context, skill: &Prepared) -> Result<Exit, Failure> {
    let Prepared {
        name,
        target,
        merge,
        ..
    } = skill;
    let repo = context.repo;
    let current = Current::read(repo, name)?;
    let hash = |dir: &Path| Snapshot::from_folder(dir).map(|folder| folder.tree_hash());
    let unchanged = (&current.source, &current.entry)
        == (&skill.current.source, &skill.current.entry)
        && match merge {
            Some(m) => hash(&repo.skill_dir(name))? == m.ours.tree_hash(),
            None => true,
        };
    if !unchanged {
        return Err(changed(name));
    }
    let plan = plan(repo, skill, &current)?;
    if up_to_date(skill, &plan) {
        report(skill, &plan, false);
        return Ok(Exit::Success);
    }
    let Some(merge) = merge else {
        context.staging()?;
        let backup = advance_records(context, name, &current, &plan)?;
        report(skill, &plan, false);
        out!("saved the records of {name} to {}", backup.display());
        return Ok(Exit::Success);
    };
    let staging = context.staging()?;
    let work = Workspace::new(staging, name);
    work.stage(&merge.merged, &target.snapshot)?;
    let (backup, old_folders_saved) = swap(
        context,
        &work,
        name,
        &current,
        &merge.ours.tree_hash(),
        &plan,
    )?;
    report(skill, &plan, false);
    let saved = if old_folders_saved {
        "the old folders and records"
    } else {
        "the records"
    };
    out!("saved {saved} of {name} to {}", backup.display());
    print_conflicts(name, &merge.conflicts, false);
    Ok(match merge.conflicts.is_empty() {
        true => Exit::Success,
        false => Exit::NeedsAttention,
    })
}

/// The conflicts a merge wrote into `skills/<name>` (`dry_run`: would write).
fn print_conflicts(name: &str, conflicts: &[Conflict], dry_run: bool) {
    if conflicts.is_empty() {
        return;
    }
    match dry_run {
        true => out!("skill {name}: the update would write these conflicts into skills/{name}:"),
        false => out!(
            "skill {name}: resolve these conflicts in skills/{name}, then delete the conflict records:"
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

/// Saves the skill's `skiman.toml` table and lock entry as
/// `repo/skiman.toml.<name>.fragment` and `repo/skiman.lock.<name>.fragment`
/// into a fresh backup folder of its own.
fn save_fragments(paths: &Paths, name: &str, current: &Current) -> Result<Backups, Failure> {
    let failed = |e: String| {
        Failure::operational(format!(
            "cannot save the records of {name} to a backup: {e}; nothing of {name} changed"
        ))
    };
    let mut backups = Backups::new(paths);
    let nothing = |e| Failure::operational(format!("{e}; nothing of {name} changed"));
    let dir = backups.dir().map_err(nothing)?;
    for (file, text) in [
        (rules::MANIFEST, &current.manifest_text),
        (rules::LOCKFILE, &current.lock_text),
    ] {
        let fragment = manifest::skill_fragment(text, name).map_err(|e| failed(e.to_string()))?;
        let rel = PathBuf::from(format!("repo/{file}.{name}.fragment"));
        let cannot = |e| failed(format!("cannot write {}: {e}", dir.join(&rel).display()));
        backups.save(&rel, fragment.as_bytes()).map_err(cannot)?;
    }
    Ok(backups)
}

/// The skill's files stay as they are; only its records move to the new
/// version: fragments to a fresh backup, then the records `plan` writes.
fn advance_records(
    context: &Context,
    name: &str,
    current: &Current,
    plan: &Plan,
) -> Result<PathBuf, Failure> {
    let backup = save_fragments(context.paths, name, current)?;
    let backup = backup
        .created()
        .expect("the fragments went in")
        .to_path_buf();
    let mut completed = Vec::new();
    let failed = |e, completed: &[String]| stopped(e, completed, context.repo, name, &backup, &[]);
    if let Some(text) = &plan.manifest {
        write_record(&context.repo.manifest_file(), text).map_err(|e| failed(e, &completed))?;
        completed.push(format!(
            "wrote the new `source.ref` and `ref_kind` of {name} to skiman.toml"
        ));
    }
    test_hook::at(name, "before-lock");
    write_record(&context.repo.lockfile(), &plan.lock).map_err(|e| failed(e, &completed))?;
    Ok(backup)
}

/// A failure before the lock entry was written (spec Merge semantics,
/// clarification 1): `failure` with the completed steps, the backup folder
/// and the recovery. `swapped_out` holds each folder the swap already
/// renamed into `tmp/<timestamp>/old/` (path in the skill repo, old folder).
/// Recovery moves aside what the completed steps put in place, puts back the
/// old folders, pastes the two fragments back and runs `skiman sync`.
fn stopped(
    failure: Failure,
    completed: &[String],
    repo: &SkillRepo,
    name: &str,
    dir: &Path,
    swapped_out: &[(String, &Path)],
) -> Failure {
    let mut message = failure.message;
    for step in completed {
        message.push_str(&format!("\n  done: {step}"));
    }
    message.push_str(&format!(
        "\n  backup folder: {}\n  to undo the update of {name}, in this order:",
        dir.display()
    ));
    let mut steps = Vec::new();
    for (place, _) in swapped_out.iter().rev() {
        steps.push(format!(
            "move {} (if it exists) to {}",
            repo.root().join(place).display(),
            dir.join("aside").join(place).display()
        ));
    }
    let first_put_back = steps.len() + 1;
    for (place, old) in swapped_out {
        steps.push(format!(
            "move {} to {}",
            old.display(),
            repo.root().join(place).display()
        ));
    }
    let last_put_back = steps.len();
    steps.push(format!(
        "in {}, replace the [skills.{name}] table and its subtables with {}",
        rules::MANIFEST,
        dir.join(format!("repo/{}.{name}.fragment", rules::MANIFEST))
            .display()
    ));
    steps.push(format!(
        "in {}, replace the [skills.{name}] entry with {}",
        rules::LOCKFILE,
        dir.join(format!("repo/{}.{name}.fragment", rules::LOCKFILE))
            .display()
    ));
    steps.push("run `skiman sync`".to_owned());
    for (i, step) in steps.iter().enumerate() {
        message.push_str(&format!("\n  {}. {step}", i + 1));
    }
    if let Some((_, old)) = swapped_out.first() {
        // `tmp/<timestamp>/old/`, as a later command keeps it below `<path>/`.
        let old_dir = old
            .parent()
            .and_then(Path::parent)
            .and_then(|d| d.strip_prefix(repo.tmp_dir()).ok())
            .expect("old folders are in tmp/<timestamp>/old/");
        let which = if first_put_back == last_put_back {
            format!("step {first_put_back}")
        } else {
            format!("steps {first_put_back} and {last_put_back}")
        };
        message.push_str(&format!(
            "\n  if a later command moved the tmp leftovers to a backup (it printed `moved tmp leftovers to <path>`), take the folders of {which} from <path>/{}/ instead",
            old_dir.display()
        ));
    }
    Failure::operational(message)
}

fn write_record(path: &Path, text: &str) -> Result<(), Failure> {
    write_atomic(path, text.as_bytes())
        .map_err(|e| Failure::operational(format!("cannot write {}: {e}", path.display())))
}

/// One skill's disposable staging inside the command's `tmp/<timestamp>/`:
/// `new/skills/<name>`, `new/upstream/<name>` (the merged skill and the new
/// snapshot, renamed into place by the swap). Removed when dropped; the
/// swapped-out folders in `old/` are not part of it.
struct Workspace {
    new_skill: PathBuf,
    new_upstream: PathBuf,
    old_skill: PathBuf,
    old_upstream: PathBuf,
}

impl Workspace {
    fn new(staging: &Staging, name: &str) -> Workspace {
        let dir = staging.dir();
        Workspace {
            new_skill: dir.join("new/skills").join(name),
            new_upstream: dir.join("new/upstream").join(name),
            old_skill: staging.old_dir().join("skills").join(name),
            old_upstream: staging.old_dir().join("upstream").join(name),
        }
    }

    fn stage(&self, merged: &Snapshot, snapshot: &Snapshot) -> Result<(), Failure> {
        fsutil::write_files(&self.new_skill, &merged.files)?;
        fsutil::write_files(&self.new_upstream, &snapshot.files)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        for dir in [&self.new_skill, &self.new_upstream] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
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
enum Change {
    Added,
    Deleted,
    Modified,
}

impl Change {
    fn verb(self) -> &'static str {
        match self {
            Change::Added => "added",
            Change::Deleted => "deleted",
            Change::Modified => "modified",
        }
    }
}

/// The files of the skill the merge changed (content or executable bit),
/// sorted by path.
fn changes(ours: &Snapshot, merged: &Snapshot) -> Vec<(Change, String)> {
    let mut out = Vec::new();
    for file in &merged.files {
        match ours.file(&file.path) {
            None => out.push((Change::Added, file.path.clone())),
            Some(old) if old != file => out.push((Change::Modified, file.path.clone())),
            Some(_) => {}
        }
    }
    for file in &ours.files {
        if merged.file(&file.path).is_none() {
            out.push((Change::Deleted, file.path.clone()));
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
/// are warnings. The conflict representation is excused: unresolved-conflict
/// violations in the skill, the frontmatter rules on a `SKILL.md` whose
/// markers start inside its frontmatter, and a missing `SKILL.md` when the
/// local side deleted it and upstream's version is in the conflict record.
fn validate(
    repo: &SkillRepo,
    name: &str,
    merged: &Snapshot,
    snapshot: &Snapshot,
    plan: &Plan,
    conflicts: &[Conflict],
) -> Result<(), Failure> {
    let skill = format!("skills/{name}");
    let upstream = format!("upstream/{name}");
    let mut files = Vec::new();
    let mut executable = BTreeSet::new();
    for (dir, snap) in [(&skill, merged), (&upstream, snapshot)] {
        for file in &snap.files {
            let path = format!("{dir}/{}", file.path);
            if file.executable {
                executable.insert(path.clone());
            }
            files.push((path, file.content.clone()));
        }
    }
    files.push((rules::LOCKFILE.to_owned(), plan.lock.as_bytes().to_vec()));
    if let Some(text) = &plan.manifest {
        files.push((rules::MANIFEST.to_owned(), text.as_bytes().to_vec()));
    }
    let working_tree = WorkingTree::new(repo.root());
    let candidate = Overlay::new(&working_tree, files)
        .with_executable(executable)
        .replacing(vec![skill.clone(), upstream]);
    path_safety::check(&candidate, &skill, "the merge result", Content::Output)?;
    let names = BTreeSet::from([name.to_owned()]);
    let scope = Scope {
        files: plan
            .changes
            .iter()
            .filter(|(change, _)| *change != Change::Deleted)
            .map(|(_, path)| format!("{skill}/{path}"))
            .collect(),
        content_only_skills: names.clone(),
        upstreams: names.clone(),
        lock_entries: names.clone(),
        consistency: names,
        ..Scope::default()
    };
    let skill_md = format!("{skill}/SKILL.md");
    let frontmatter_markers = conflicts.iter().any(|c| {
        c.kind == ConflictKind::Markers
            && c.path == "SKILL.md"
            && merged
                .file("SKILL.md")
                .is_some_and(|f| markers_in_frontmatter(&f.content))
    });
    let skill_md_upstream = conflicts
        .iter()
        .any(|c| matches!(c.kind, ConflictKind::UpstreamVersion { .. }) && c.path == "SKILL.md");
    let inside = format!("{skill}/");
    let excused = |v: &Violation| {
        (skill_md_upstream && v.rule == rule::SKILL_MD && v.path == skill)
            || (v.path.starts_with(&inside)
                && (v.rule == rule::UNRESOLVED_CONFLICT
                    || (frontmatter_markers
                        && v.path == skill_md
                        && FRONTMATTER_RULES.contains(&v.rule))))
    };
    rules::validate_write_except(&candidate, &scope, &excused)
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

/// Swaps the staged merge result and snapshot in (spec Merge semantics):
/// re-hash `skills/<name>`, save the fragments, re-hash again, rename the
/// current folders to `tmp/<timestamp>/old/`, rename the new ones into
/// place, write the changed `source.ref` and `ref_kind`, write the lock
/// entry (always the last write for the skill), then move the old folders
/// into the same backup. Returns the backup folder and whether both old
/// folders reached it (else they are tmp leftovers).
fn swap(
    context: &Context,
    work: &Workspace,
    name: &str,
    current: &Current,
    start_hash: &str,
    plan: &Plan,
) -> Result<(PathBuf, bool), Failure> {
    let repo = &context.repo;
    let skill_dir = repo.skill_dir(name);
    let upstream_dir = repo.upstream_dir(name);
    let unchanged = |backup: Option<&Path>| -> Result<(), Failure> {
        let now = tree_hash_of_dir(&skill_dir).map_err(|e| {
            Failure::operational(format!("cannot read {}: {e}", skill_dir.display()))
        })?;
        if now == start_hash {
            return Ok(());
        }
        let saved = backup.map_or_else(String::new, |dir| {
            format!(" (its records were saved to {})", dir.display())
        });
        Err(Failure::operational(format!(
            "skills/{name} changed during the update, so the update of {name} stopped and nothing was swapped{saved}; your newer edits are kept"
        )))
    };
    test_hook::at(name, "before-rehash-1");
    unchanged(None)?;
    let mut backups = save_fragments(context.paths, name, current)?;
    let backup = backups
        .created()
        .expect("the fragments went in")
        .to_path_buf();
    test_hook::at(name, "before-rehash-2");
    unchanged(Some(&backup))?;

    let mut completed: Vec<String> = Vec::new();
    let steps: [(&Path, &Path); 4] = [
        (&skill_dir, &work.old_skill),
        (&upstream_dir, &work.old_upstream),
        (&work.new_upstream, &upstream_dir),
        (&work.new_skill, &skill_dir),
    ];
    let failed = |e: Failure, completed: &[String]| {
        let swapped_out = [
            (format!("skills/{name}"), work.old_skill.as_path()),
            (format!("upstream/{name}"), work.old_upstream.as_path()),
        ];
        let swapped_out = &swapped_out[..completed.len().min(2)];
        stopped(e, completed, repo, name, &backup, swapped_out)
    };
    for (i, (from, to)) in steps.into_iter().enumerate() {
        test_hook::at(name, &format!("before-rename-{}", i + 1));
        let staging = context.staging.as_ref().expect("begun before staging");
        staging.place(from, to).map_err(|e| failed(e, &completed))?;
        completed.push(format!("renamed {} to {}", from.display(), to.display()));
    }
    if let Some(text) = &plan.manifest {
        write_record(&repo.manifest_file(), text).map_err(|e| failed(e, &completed))?;
        completed.push(format!(
            "wrote the new `source.ref` and `ref_kind` of {name} to skiman.toml"
        ));
    }
    test_hook::at(name, "before-lock");
    write_record(&repo.lockfile(), &plan.lock).map_err(|e| failed(e, &completed))?;

    test_hook::at(name, "before-backup-move");
    let mut all_saved = true;
    for (old, rel) in [
        (&work.old_skill, format!("repo/skills/{name}")),
        (&work.old_upstream, format!("repo/upstream/{name}")),
    ] {
        if let Err(e) = backups.move_in(old, Path::new(&rel)) {
            err!(
                "warning: skill {name}: cannot move {} to the backup ({e}); it stays in .skiman/tmp/ as a tmp leftover",
                old.display()
            );
            all_saved = false;
        }
    }
    Ok((backup, all_saved))
}
