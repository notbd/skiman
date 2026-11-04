//! `skiman update [names]`: fetch each third-party skill's new upstream
//! version, merge it three-way with the local modifications, and swap the
//! result in (spec Merge semantics). Without names, every third-party skill.
//! Each skill is processed on its own; the exit code is the highest per-skill
//! result. Ends with a sync; never commits.
//!
//! One skill goes through these steps (`update_skill`); later tickets hook in
//! where marked:
//!
//! 1. [`Current::read`]: the skill's records; refusals before anything is
//!    fetched (unresolved conflicts, a snapshot that differs from the lock).
//! 2. [`fetch`]: the version to move to (spec Version semantics) and its
//!    snapshot; a local folder is re-read.
//! 3. Same snapshot: up to date, or [`advance_records`] (fragments to a fresh
//!    backup, then `source.ref` and `ref_kind`, then the lock entry).
//! 4. Otherwise [`merge`] in `tmp/<timestamp>/merge/<name>/` (conflicts are
//!    written into the result, exit 3), [`validate`] the candidate output
//!    (path safety, rules), stage it in `tmp/<timestamp>/new/`,
//!    then [`swap`]: re-hash, fragments, re-hash, renames through
//!    `tmp/<timestamp>/old/`, `source.ref` and `ref_kind`, lock entry (last
//!    write), old folders to the backup; a failure before the lock entry
//!    prints the recovery ([`stopped`]).

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::Exit;
use crate::backup::Backup;
use crate::commands::has_terminal;
use crate::failure::{Failure, finish};
use crate::fetch::{Cache, RefKind, ResolvedRef};
use crate::fsutil::write_atomic;
use crate::git::Prompts;
use crate::lockfile::{self, LockEntry, Lockfile};
use crate::manifest::{self, Manifest, SourceRecord};
use crate::merge::{Conflict, ConflictKind, MergeRepo, Outcome};
use crate::path_safety::{self, Content};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, CONFLICT_MARKERS, FRONTMATTER_RULES, Scope, Violation, rule};
use crate::snapshot::{Snapshot, SnapshotFile};
use crate::source::{Source, SourceKind};
use crate::staging::{self, Staging};
use crate::sync::{self, Output};
use crate::test_hook;
use crate::tree::{Overlay, WorkingTree, tree_hash_of_dir};

pub fn run(names: Vec<String>, git_ref: Option<String>) -> Exit {
    finish(update(names, git_ref))
}

/// What every skill's update shares.
struct Context {
    paths: Paths,
    repo: SkillRepo,
    lock: RepoLock,
    prompts: Prompts,
    /// The command's `tmp/<timestamp>/`, begun when the first skill needs a merge.
    staging: Option<Staging>,
}

impl Context {
    fn staging(&mut self) -> Result<&Staging, Failure> {
        if self.staging.is_none() {
            self.staging = Some(Staging::begin(&self.paths, &self.repo, &self.lock)?);
        }
        Ok(self.staging.as_ref().expect("just begun"))
    }
}

fn update(names: Vec<String>, git_ref: Option<String>) -> Result<Exit, Failure> {
    if git_ref.is_some() && names.len() != 1 {
        return Err(Failure::usage("--ref needs exactly one skill name"));
    }
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let prompts = if has_terminal() {
        Prompts::Allowed
    } else {
        Prompts::Never
    };
    let lock = RepoLock::wait(&repo)?;
    let names = if names.is_empty() {
        let manifest = Manifest::load(&repo)?;
        let names: Vec<String> = manifest
            .skills()
            .into_keys()
            .filter(|name| manifest.source(name).is_some())
            .collect();
        if names.is_empty() {
            println!("no third-party skills to update");
        }
        names
    } else {
        let mut seen = BTreeSet::new();
        names
            .into_iter()
            .filter(|name| seen.insert(name.clone()))
            .collect()
    };

    let mut context = Context {
        paths,
        repo,
        lock,
        prompts,
        staging: None,
    };
    let mut exit = Exit::Success;
    let mut any_done = names.is_empty();
    for name in &names {
        match update_skill(&mut context, name, git_ref.as_deref()) {
            Ok(result) => {
                any_done = true;
                exit = exit.combine(result);
            }
            Err(failure) => {
                eprintln!("error: skill {name}: {}", failure.message);
                exit = exit.combine(failure.exit);
            }
        }
    }
    let Context {
        paths,
        repo,
        lock,
        staging,
        ..
    } = context;
    drop(staging);
    if any_done {
        exit = exit.combine(sync::run(&paths, &repo, &lock, Output::Normal));
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

fn read_text(path: &Path) -> Result<Option<String>, Failure> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Failure::operational(format!(
            "cannot read {}: {e}",
            path.display()
        ))),
    }
}

/// The version an update moves to.
struct Target {
    /// The commit, or for a local folder the snapshot's tree hash.
    version: String,
    snapshot: Snapshot,
    /// The new lock entry.
    entry: LockEntry,
    /// `skiman.toml` with the new `source.ref` and `ref_kind`; `None` when
    /// they stay the same.
    manifest_text: Option<String>,
    /// `  note: ...` lines printed after the skill's result.
    notes: Vec<String>,
}

/// Updates one skill; `NeedsAttention` when conflicts were written.
fn update_skill(context: &mut Context, name: &str, git_ref: Option<&str>) -> Result<Exit, Failure> {
    let current = Current::read(&context.repo, name)?;
    let target = fetch(context, name, &current, git_ref)?;
    let result = update_to(context, name, &current, &target)?;
    for note in &target.notes {
        println!("  note: {note}");
    }
    Ok(result)
}

/// Moves the skill to `target`: records only when its snapshot is the same,
/// else merge, validate and swap.
fn update_to(
    context: &mut Context,
    name: &str,
    current: &Current,
    target: &Target,
) -> Result<Exit, Failure> {
    if target.entry.snapshot == current.entry.snapshot {
        if target.entry == current.entry && target.manifest_text.is_none() {
            println!("skill {name}: up to date at {}", target.version);
            return Ok(Exit::Success);
        }
        let backup = advance_records(context, name, current, target)?;
        println!(
            "skill {name}: now at {}; no file of the skill changed\nsaved the records of {name} to {}",
            target.version,
            backup.dir().display()
        );
        return Ok(Exit::Success);
    }

    let ours = Snapshot::from_folder(&context.repo.skill_dir(name))?;
    let base = Snapshot::from_folder(&context.repo.upstream_dir(name))?;
    let staging = context.staging()?;
    let work = Workspace::new(staging, name);
    let (merged, conflicts) = match merge(&work, &base, &ours, &target.snapshot)? {
        Outcome::Clean(merged) => (merged, Vec::new()),
        Outcome::Conflicts { merged, conflicts } => (merged, conflicts),
    };
    let changes = changes(&ours, &merged);
    let lock_text = lockfile::with_entry(&current.lock_text, name, &target.entry)
        .map_err(|e| Failure::refused(format!("skiman.lock: {e}")))?;
    let records = Records {
        manifest: target.manifest_text.as_deref(),
        lock: &lock_text,
    };
    validate(
        &context.repo,
        name,
        &merged,
        &target.snapshot,
        &records,
        &changes,
        &conflicts,
    )?;
    work.stage(&merged, &target.snapshot)?;
    let (backup, old_folders_saved) =
        swap(context, &work, name, current, &ours.tree_hash(), &records)?;

    let with_conflicts = if conflicts.is_empty() {
        ""
    } else {
        " with conflicts"
    };
    println!(
        "skill {name}: updated to {}{with_conflicts}; changed files:",
        target.version
    );
    for (change, path) in &changes {
        println!("  {} {path}", change.verb());
    }
    let saved = if old_folders_saved {
        "the old folders and records"
    } else {
        "the records"
    };
    println!("saved {saved} of {name} to {}", backup.display());
    if conflicts.is_empty() {
        return Ok(Exit::Success);
    }
    println!(
        "skill {name}: resolve these conflicts in skills/{name}, then delete the conflict records:"
    );
    for conflict in &conflicts {
        println!("  {}", describe(conflict));
    }
    Ok(Exit::NeedsAttention)
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
/// `ref_kind` is always derived, never read.
fn fetch(
    context: &Context,
    name: &str,
    current: &Current,
    requested: Option<&str>,
) -> Result<Target, Failure> {
    let source = &current.source;
    if source.url.starts_with(['/', '~', '.']) {
        return fetch_local(context, name, current, requested);
    }
    let cache = Cache::open(&context.paths, &source.url, context.prompts)?;
    let mut notes = Vec::new();
    let (resolved, commit) = match requested.or(source.git_ref.as_deref()) {
        None => (None, cache.default_branch()?.commit),
        Some(git_ref) => {
            let resolved = cache.resolve_ref(git_ref)?;
            let changing = requested.is_some() || source.git_ref != current.entry.git_ref;
            let commit = pick_commit(name, &resolved, current, changing, &mut notes);
            (Some(resolved), commit)
        }
    };
    cache.fetch_commit(&commit)?;
    let snapshot = Snapshot::read(&cache, &commit, &source.path)?;
    let entry = LockEntry {
        commit: Some(commit.clone()),
        git_ref: resolved.as_ref().map(|r| r.name.clone()),
        snapshot: snapshot.tree_hash(),
    };
    Ok(Target {
        version: commit,
        snapshot,
        entry,
        manifest_text: new_manifest_text(name, current, resolved.as_ref())?,
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

/// A local-folder source: the folder is re-read as it is now and versioned
/// by its tree hash; it has no refs. A missing folder is an error (exit 4)
/// that keeps the current content.
fn fetch_local(
    context: &Context,
    name: &str,
    current: &Current,
    requested: Option<&str>,
) -> Result<Target, Failure> {
    let source = &current.source;
    if requested.is_some() {
        return Err(Failure::refused(format!(
            "{} is a local folder, which has no versions; drop --ref",
            source.url
        )));
    }
    if source.git_ref.is_some() {
        return Err(Failure::refused(format!(
            "{} is a local folder, which has no versions; remove `ref` from [skills.{name}.source] in skiman.toml",
            source.url
        )));
    }
    let parsed = Source::parse(&source.url, context.paths.home(), Path::new("/"))?;
    let SourceKind::Local { dir } = parsed.kind else {
        unreachable!("a `/`, `~` or `.` source is a local folder");
    };
    let dir = if source.path.is_empty() {
        dir
    } else {
        dir.join(&source.path)
    };
    if !dir.is_dir() {
        return Err(Failure::operational(format!(
            "the local folder {} does not exist; nothing of {name} changed",
            dir.display()
        )));
    }
    let snapshot = Snapshot::read_local(&dir)?;
    let entry = LockEntry {
        commit: None,
        git_ref: None,
        snapshot: snapshot.tree_hash(),
    };
    Ok(Target {
        version: entry.snapshot.clone(),
        snapshot,
        entry,
        manifest_text: new_manifest_text(name, current, None)?,
        notes: Vec::new(),
    })
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

/// Saves the skill's `skiman.toml` table and lock entry as
/// `repo/skiman.toml.<name>.fragment` and `repo/skiman.lock.<name>.fragment`
/// into a fresh backup folder of its own.
fn save_fragments(paths: &Paths, name: &str, current: &Current) -> Result<Backup, Failure> {
    let failed = |e: String| {
        Failure::operational(format!(
            "cannot save the records of {name} to a backup: {e}; nothing of {name} changed"
        ))
    };
    let backup = Backup::create(paths).map_err(|e| failed(e.to_string()))?;
    let repo_dir = backup.dir().join("repo");
    std::fs::create_dir_all(&repo_dir).map_err(|e| failed(e.to_string()))?;
    for (file, text) in [
        (rules::MANIFEST, &current.manifest_text),
        (rules::LOCKFILE, &current.lock_text),
    ] {
        let fragment = manifest::skill_fragment(text, name).map_err(|e| failed(e.to_string()))?;
        let path = repo_dir.join(format!("{file}.{name}.fragment"));
        std::fs::write(&path, fragment)
            .map_err(|e| failed(format!("cannot write {}: {e}", path.display())))?;
    }
    Ok(backup)
}

/// The skill's files stay as they are; only its records move to the new
/// version: fragments to a fresh backup, then the lock entry.
fn advance_records(
    context: &Context,
    name: &str,
    current: &Current,
    target: &Target,
) -> Result<Backup, Failure> {
    let lock_text = lockfile::with_entry(&current.lock_text, name, &target.entry)
        .map_err(|e| Failure::refused(format!("skiman.lock: {e}")))?;
    let backup = save_fragments(&context.paths, name, current)?;
    let mut completed = Vec::new();
    let failed = |e, completed: &[String]| stopped(e, completed, &context.repo, name, &backup, &[]);
    if let Some(text) = &target.manifest_text {
        write_record(&context.repo.manifest_file(), text).map_err(|e| failed(e, &completed))?;
        completed.push(format!(
            "wrote the new `source.ref` and `ref_kind` of {name} to skiman.toml"
        ));
    }
    test_hook::at(name, "before-lock");
    write_record(&context.repo.lockfile(), &lock_text).map_err(|e| failed(e, &completed))?;
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
    backup: &Backup,
    swapped_out: &[(String, &Path)],
) -> Failure {
    let mut message = failure.message;
    for step in completed {
        message.push_str(&format!("\n  done: {step}"));
    }
    let dir = backup.dir();
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

/// The skill's new `skiman.toml` (when its `source.ref` or `ref_kind`
/// changes) and `skiman.lock` texts.
struct Records<'a> {
    manifest: Option<&'a str>,
    lock: &'a str,
}

fn write_record(path: &Path, text: &str) -> Result<(), Failure> {
    write_atomic(path, text.as_bytes())
        .map_err(|e| Failure::operational(format!("cannot write {}: {e}", path.display())))
}

/// One skill's disposable staging inside the command's `tmp/<timestamp>/`:
/// `merge/<name>/` (the temporary merge repository) and
/// `new/skills/<name>`, `new/upstream/<name>` (the merged skill and the new
/// snapshot, renamed into place by the swap). Removed when dropped; the
/// swapped-out folders in `old/` are not part of it.
struct Workspace {
    merge_dir: PathBuf,
    new_skill: PathBuf,
    new_upstream: PathBuf,
    old_skill: PathBuf,
    old_upstream: PathBuf,
}

impl Workspace {
    fn new(staging: &Staging, name: &str) -> Workspace {
        let dir = staging.dir();
        Workspace {
            merge_dir: dir.join("merge").join(name),
            new_skill: dir.join("new/skills").join(name),
            new_upstream: dir.join("new/upstream").join(name),
            old_skill: staging.old_dir().join("skills").join(name),
            old_upstream: staging.old_dir().join("upstream").join(name),
        }
    }

    fn stage(&self, merged: &Snapshot, snapshot: &Snapshot) -> Result<(), Failure> {
        write_files(&self.new_skill, &merged.files)?;
        write_files(&self.new_upstream, &snapshot.files)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        for dir in [&self.merge_dir, &self.new_skill, &self.new_upstream] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn write_files(dir: &Path, files: &[SnapshotFile]) -> Result<(), Failure> {
    std::fs::create_dir_all(dir)
        .map_err(|e| Failure::operational(format!("cannot create {}: {e}", dir.display())))?;
    for file in files {
        let path = dir.join(&file.path);
        let io = |e: std::io::Error| {
            Failure::operational(format!("cannot write {}: {e}", path.display()))
        };
        std::fs::create_dir_all(path.parent().expect("file has a parent")).map_err(io)?;
        std::fs::write(&path, &file.content).map_err(io)?;
        let mode = if file.executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).map_err(io)?;
    }
    Ok(())
}

/// base = current `upstream/<name>`, ours = current `skills/<name>` (working
/// tree, including uncommitted edits), theirs = the new snapshot.
fn merge(
    work: &Workspace,
    base: &Snapshot,
    ours: &Snapshot,
    theirs: &Snapshot,
) -> Result<Outcome, Failure> {
    std::fs::create_dir_all(&work.merge_dir).map_err(|e| {
        Failure::operational(format!("cannot create {}: {e}", work.merge_dir.display()))
    })?;
    let repo = MergeRepo::create(&work.merge_dir)?;
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
/// get every rule; every file of the merged skill and of the new snapshot
/// gets the secret, symlink and size rules; the lock entry and Consistency
/// for the skill. Violations elsewhere, including the other rules on files
/// the merge did not change, are warnings. The conflict representation is
/// excused: unresolved-conflict violations in the skill, and the frontmatter
/// rules on a `SKILL.md` whose markers start inside its frontmatter.
fn validate(
    repo: &SkillRepo,
    name: &str,
    merged: &Snapshot,
    snapshot: &Snapshot,
    records: &Records,
    changes: &[(Change, String)],
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
    files.push((rules::LOCKFILE.to_owned(), records.lock.as_bytes().to_vec()));
    if let Some(text) = records.manifest {
        files.push((rules::MANIFEST.to_owned(), text.as_bytes().to_vec()));
    }
    let working_tree = WorkingTree::new(repo.root());
    let candidate = Overlay::new(&working_tree, files)
        .with_executable(executable)
        .replacing(vec![skill.clone(), upstream]);
    path_safety::check(&candidate, &skill, "the merge result", Content::Output)?;
    let names = BTreeSet::from([name.to_owned()]);
    let scope = Scope {
        files: changes
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
    let inside = format!("{skill}/");
    let excused = |v: &Violation| {
        v.path.starts_with(&inside)
            && (v.rule == rule::UNRESOLVED_CONFLICT
                || (frontmatter_markers
                    && v.path == skill_md
                    && FRONTMATTER_RULES.contains(&v.rule)))
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
    records: &Records,
) -> Result<(PathBuf, bool), Failure> {
    let repo = &context.repo;
    let skill_dir = repo.skill_dir(name);
    let upstream_dir = repo.upstream_dir(name);
    let unchanged = |backup: Option<&Backup>| -> Result<(), Failure> {
        let now = tree_hash_of_dir(&skill_dir).map_err(|e| {
            Failure::operational(format!("cannot read {}: {e}", skill_dir.display()))
        })?;
        if now == start_hash {
            return Ok(());
        }
        let saved = backup.map_or_else(String::new, |b| {
            format!(" (its records were saved to {})", b.dir().display())
        });
        Err(Failure::operational(format!(
            "skills/{name} changed during the update, so the update of {name} stopped and nothing was swapped{saved}; your newer edits are kept"
        )))
    };
    test_hook::at(name, "before-rehash-1");
    unchanged(None)?;
    let backup = save_fragments(&context.paths, name, current)?;
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
        staging::rename_into_place(from, to).map_err(|e| failed(e, &completed))?;
        completed.push(format!("renamed {} to {}", from.display(), to.display()));
    }
    if let Some(text) = records.manifest {
        write_record(&repo.manifest_file(), text).map_err(|e| failed(e, &completed))?;
        completed.push(format!(
            "wrote the new `source.ref` and `ref_kind` of {name} to skiman.toml"
        ));
    }
    test_hook::at(name, "before-lock");
    write_record(&repo.lockfile(), records.lock).map_err(|e| failed(e, &completed))?;

    test_hook::at(name, "before-backup-move");
    let mut all_saved = true;
    for (old, rel) in [
        (&work.old_skill, format!("repo/skills/{name}")),
        (&work.old_upstream, format!("repo/upstream/{name}")),
    ] {
        if let Err(e) = backup.move_in(old, Path::new(&rel)) {
            eprintln!(
                "warning: skill {name}: cannot move {} to the backup ({e}); it stays in .skiman/tmp/ as a tmp leftover",
                old.display()
            );
            all_saved = false;
        }
    }
    Ok((backup.dir().to_path_buf(), all_saved))
}
