//! `skiman import <folder>`: bring an existing setup in from a prepared
//! import folder (ADR 0001), and `skiman import --guide`, the guide an agent
//! follows to prepare one (read-only).
//!
//! Order: no TTY and no `--yes` -> exit 2 -> config -> the canon's
//! `skiman.toml`, `skiman.lock` and shared body are regular files, and it is
//! initialised -> phase 1 on the folder (layout, path safety, no `[linked]`
//! table, every rule except Consistency) -> the canon's linked folders
//! ([`linked_folders`]: none unreadable, none of their names, no write into
//! one) -> whether the canon already holds the whole import
//! ([`progress`]; if so, nothing is fetched or written and the run goes on
//! at the takeover plan) -> fetch every declared source -> phase 2
//! on the complete result (Consistency, secrets, everything) -> what the
//! canon holds against what the import writes ([`canon_todo`]) ->
//! takeover plan, no special file in it and no local source it moves away
//! -> preview -> confirm -> canon lock, checks again (the takeover plan
//! included) -> tmp leftovers -> `skiman.toml`, `skiman.lock`, `upstream/`,
//! `instructions/shared.md`, `skills/`, each not yet in place ([`crate::change`],
//! which backs up what it replaces) -> originals in tool directories and instruction
//! locations to the backup -> sync, which creates every link. Never commits.
//!
//! There is no record of a run (ADR 0005). An import that stopped part-way
//! is finished by running the same command again: every run works out the
//! whole import again, keeps what is already in place and does the rest.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};
use unicode_normalization::UnicodeNormalization;

use crate::Exit;
use crate::backup::{self, Backups};
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::change::{self, Change, Held, Want};
use crate::commands::{confirm, has_terminal, prompts};
use crate::failure::{Failure, finish};
use crate::fetch::Cache;
use crate::git::Prompts;
use crate::import_guide;
use crate::layout::{Layout, Placed};
use crate::linked::{Linked, State};
use crate::lockfile::{self, LockEntry, Lockfile};
use crate::manifest::{self, Manifest, SkillEntry, SourceRecord};
use crate::ownership::{Entry, Ownership};
use crate::path_safety::{self, Content};
use crate::paths::Paths;
use crate::project::item::MIRROR_DIR;
use crate::rules::{self, Scope};
use crate::skiman_manual;
use crate::snapshot::Snapshot;
use crate::source::{UrlKind, check_url};
use crate::staging::Staging;
use crate::sync::{self, Output, Place};
use crate::test_hook;
use crate::tree::{self, Kind, Overlay, Tree, WorkingTree};
use crate::upstream::{self, Upstream, print_not_installed};

pub fn guide() -> Exit {
    finish(
        std::io::stdout()
            .write_all(import_guide::text().as_bytes())
            .map(|()| Exit::Success)
            .map_err(|e| Failure::operational(format!("cannot print the guide: {e}"))),
    )
}

/// `folder` defaults to `<canon>/.skiman/import/`.
pub fn run(folder: Option<&Path>, yes: bool) -> Exit {
    finish(import(folder, yes))
}

const SHARED_BODY: &str = "instructions/shared.md";

/// The canon's files the import replaces whole, each a regular file.
const CANON_FILES: [&str; 3] = [rules::MANIFEST, rules::LOCKFILE, SHARED_BODY];

fn import(folder: Option<&Path>, yes: bool) -> Result<Exit, Failure> {
    let terminal = has_terminal();
    if !terminal && !yes {
        return Err(Failure::usage(
            "missing flag without a terminal: --yes (import without asking for confirmation)",
        ));
    }
    let paths = Paths::from_env()?;
    let repo = Canon::from_config(&paths)?;
    let folder = folder.map_or_else(|| repo.default_import_dir(), Path::to_path_buf);
    let folder = folder.canonicalize().map_err(|e| {
        Failure::refused(format!(
            "the import folder {} is not reachable: {e}",
            folder.display()
        ))
    })?;
    refuse_irregular_files(&repo)?;
    // Which way the run goes, the skiman.toml it writes and the one its
    // takeover follows come from these as first read: under the lock, any
    // byte of them that changed refuses.
    let canon_files = || CANON_FILES.map(|rel| std::fs::read(repo.root().join(rel)).ok());
    let first_read = canon_files();
    let repo_manifest = std::fs::read_to_string(repo.manifest_file()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Failure::refused(format!(
                "{} is not an initialised canon (it has no skiman.toml); run `skiman init {}`",
                repo.root().display(),
                repo.root().display()
            ))
        } else {
            Failure::operational(format!(
                "cannot read {}: {e}",
                repo.manifest_file().display()
            ))
        }
    })?;
    let prompts = prompts();

    let content = Folder::read(&folder)?;
    refuse_linked_tables(&content)?;
    let holds = |name: &str| content.skills.contains_key(name);
    let manifest_text = merge_canon_config(&content.manifest, &repo_manifest, holds)?;
    check_folder(&content, &manifest_text)?;
    let folder_manifest = Manifest::parse(&manifest_text)
        .map_err(|e| Failure::refused(format!("skiman.toml: {e}")))?;
    // Before anything is fetched; again with the takeover plan.
    linked_folders(&paths, &repo, &folder_manifest, &content)?;
    // `None` when the canon already holds the whole import (an earlier
    // run wrote it): nothing is fetched or written, and the takeover follows
    // the repo's own skiman.toml.
    let (writes, manifest) = match progress(&repo, &content, &folder_manifest, &repo_manifest)? {
        Progress::Done => (
            None,
            Manifest::parse(&repo_manifest).expect("parsed by progress"),
        ),
        Progress::OtherSharedBody => return Err(other_shared_body(&repo, &folder)),
        Progress::NotDone => {
            let thirds = fetch_sources(&paths, &repo, &content, &manifest_text, prompts)?;
            test_hook::at("import", "fetched");
            let manifest_text = with_ref_kinds(manifest_text, &thirds)?;
            let lock_text = lock_text(&thirds)?;
            check_result(&content, &manifest_text, &lock_text, &thirds)?;
            let manifest = Manifest::parse(&manifest_text)
                .map_err(|e| Failure::refused(format!("skiman.toml: {e}")))?;
            let candidate = Candidate {
                content: &content,
                manifest: manifest_text,
                lock: lock_text,
                thirds,
            };
            let change = candidate.change();
            let todo = canon_todo(&repo, &candidate, &change)?;
            (Some((candidate, change, todo)), manifest)
        }
    };
    let plan = Plan::make(&paths, &repo, &manifest, &content)?;
    refuse_special_files(&plan)?;
    refuse_moving_sources(&paths, &content, &manifest, &plan)?;

    out!(
        "import {} into the canon {}:",
        content.root.display(),
        repo.root().display()
    );
    match &writes {
        Some((candidate, change, todo)) => print_writes(&repo, candidate, change, todo),
        None => {
            out!(
                "  the canon already holds everything this import writes: nothing is written to it, and its skiman.toml stays as it is"
            );
            let differs = config_differences(&folder_manifest, &manifest, &content);
            if !differs.is_empty() {
                out!(
                    "  the import folder's skiman.toml differs from the canon's in {}; the canon's are kept",
                    differs.join(", ")
                );
            }
        }
    }
    print_plan(&repo, &plan, &paths);
    if !yes && !confirm(&format!("Import {}? [y/N] ", folder.display()))? {
        return Err(Failure::refused("not imported; nothing was changed"));
    }

    test_hook::at("import", "confirmed");
    let lock = CanonLock::wait(&paths, &repo)?;
    refuse_irregular_files(&repo)?;
    let changed = canon_files() != first_read
        || match &writes {
            Some((candidate, change, todo)) => canon_todo(&repo, candidate, change)? != *todo,
            None => progress(&repo, &content, &folder_manifest, &repo_manifest)? != Progress::Done,
        };
    if changed {
        return Err(Failure::refused(
            "the canon changed after this import read it; nothing was changed, run the import again",
        ));
    }
    if Folder::hash(&folder)? != content.hash {
        return Err(Failure::refused(format!(
            "the import folder {} changed after this import read it; nothing was changed, run the import again",
            folder.display()
        )));
    }
    // What moves, in which order, and what is left alone, as the preview showed.
    if Plan::make(&paths, &repo, &manifest, &content)? != plan {
        return Err(Failure::refused(
            "the tool directories or instruction files changed after the preview; nothing was changed, run the import again",
        ));
    }
    refuse_special_files(&plan)?;
    refuse_moving_sources(&paths, &content, &manifest, &plan)?;
    let mut backups = Backups::new(&paths);
    // The backup folder first, so one that cannot be made stops the run
    // before the canon is written.
    if !plan.moves.is_empty() {
        let nothing = |e| Failure::operational(format!("{e}; nothing was changed"));
        backups.dir().map_err(nothing)?;
    }
    let mut moved = false;
    let done = match &writes {
        Some((_, change, todo)) => change.write(todo, &repo, &lock, &mut backups).map(|lines| {
            for line in lines {
                out!("{line}");
            }
        }),
        // tmp leftovers move all the same.
        None => Staging::begin(&repo, &lock, &mut backups).map(drop),
    }
    .and_then(|()| take_over(&plan, &mut backups, &mut moved).map_err(change::stopped));
    let originals = match backups.created().filter(|_| moved) {
        None => "this run moved no original".to_owned(),
        Some(dir) => format!("the originals this run moved are in {}", dir.display()),
    };
    if let Err(failure) = done {
        return Err(Failure {
            message: format!("{}\n{originals}", failure.message),
            ..failure
        });
    }
    let exit = sync::run(&paths, &repo, &lock, Output::Normal);
    if writes.is_some() {
        out!("imported {}; {originals}", folder.display());
    } else {
        out!(
            "the canon {} already held this import; {originals}",
            repo.root().display()
        );
    }
    out!("the import folder {} can be deleted", folder.display());
    Ok(exit)
}

/// What the import folder holds, read once: what is checked is what is written.
struct Folder {
    root: PathBuf,
    /// Tree hash of the whole folder.
    hash: String,
    /// By skill name.
    skills: BTreeMap<String, Skill>,
    shared_body: Vec<u8>,
    manifest: String,
}

/// One skill folder of the import folder.
struct Skill {
    /// `skills/[<group>/]<name>`, the same in the canon.
    path: String,
    snapshot: Snapshot,
}

impl Folder {
    fn hash(root: &Path) -> Result<String, Failure> {
        tree::tree_hash_of_dir(root)
            .map_err(|e| Failure::operational(format!("cannot read {}: {e}", root.display())))
    }

    /// Checks the layout and path safety, then reads every file.
    fn read(root: &Path) -> Result<Folder, Failure> {
        let io = |e: std::io::Error| {
            Failure::operational(format!("cannot read {}: {e}", root.display()))
        };
        let hash = Folder::hash(root)?;
        let tree = WorkingTree::new(root);
        let mut problems = Vec::new();
        // (name, whether it is a folder)
        let expected = [
            ("skills", true),
            ("instructions", true),
            ("skiman.toml", false),
        ];
        for (name, folder) in expected {
            let kind = tree.kind(name).map_err(io)?;
            let found = if folder {
                kind == Some(Kind::Dir)
            } else {
                matches!(kind, Some(Kind::File { .. }))
            };
            if !found {
                let what = if folder { "folder" } else { "file" };
                problems.push(format!("{name} is missing or not a {what}"));
            }
        }
        for entry in tree.children("").map_err(io)? {
            if !tree::is_ignored(&entry.path) && !expected.iter().any(|(n, _)| *n == entry.path) {
                problems.push(format!("{} is not part of an import folder", entry.path));
            }
        }
        if tree.kind("instructions").map_err(io)? == Some(Kind::Dir) {
            if !matches!(tree.kind(SHARED_BODY).map_err(io)?, Some(Kind::File { .. })) {
                problems.push(format!("{SHARED_BODY} is missing or not a file"));
            }
            for entry in tree.children("instructions").map_err(io)? {
                if !tree::is_ignored(&entry.path) && entry.path != SHARED_BODY {
                    problems.push(format!("{} is not part of an import folder", entry.path));
                }
            }
        }
        let mut placed = Vec::new();
        if tree.kind("skills").map_err(io)? == Some(Kind::Dir) {
            for skill in Layout::read(&tree).map_err(io)?.skills() {
                if tree.kind(&skill.path()).map_err(io)? == Some(Kind::Dir) {
                    placed.push(skill.clone());
                } else {
                    problems.push(format!(
                        "{} is not a folder; every entry in skills/ is a skill folder or a group folder of skill folders",
                        skill.path()
                    ));
                }
            }
        }
        if !problems.is_empty() {
            let lines: Vec<String> = problems.iter().map(|p| format!("  {p}")).collect();
            return Err(Failure::refused(format!(
                "refusing to import {}: an import folder holds exactly skills/, instructions/shared.md and skiman.toml; nothing was changed:\n{}",
                root.display(),
                lines.join("\n")
            )));
        }
        let mut refusals = Vec::new();
        // Skill names, and group names, on a case- or normalisation-insensitive file system.
        let mut folded: BTreeMap<String, String> = BTreeMap::new();
        for skill in &placed {
            let key = skill.name.nfc().collect::<String>().to_lowercase();
            if let Some(other) = folded.insert(key, skill.path()) {
                let path = skill.path();
                refusals.push(if other.ends_with(&format!("/{}", skill.name)) {
                    format!(
                        "{} holds {other} and {path}, two skills named `{}`; a name may be in one place only, whatever its group",
                        root.display(),
                        skill.name
                    )
                } else {
                    format!(
                        "{} holds {other} and {path}, whose names are the same on a case- or normalisation-insensitive file system",
                        root.display()
                    )
                });
            }
        }
        let mut skills = BTreeMap::new();
        for skill in placed {
            let dir = skill.path();
            match path_safety::check(
                &tree,
                &dir,
                &root.join(&dir).display().to_string(),
                Content::Upstream,
            ) {
                Ok(()) => {
                    let snapshot = Snapshot::from_folder(&root.join(&dir))?;
                    skills.insert(
                        skill.name,
                        Skill {
                            path: dir,
                            snapshot,
                        },
                    );
                }
                Err(f) => refusals.push(f.message),
            }
        }
        if !refusals.is_empty() {
            return Err(Failure::refused(format!(
                "refusing to import {}; nothing was changed:\n{}",
                root.display(),
                refusals.join("\n")
            )));
        }
        let shared_body = tree.read(SHARED_BODY).map_err(io)?;
        let manifest = String::from_utf8(tree.read("skiman.toml").map_err(io)?).map_err(|_| {
            Failure::refused(format!(
                "refusing to import {}: skiman.toml: toml-syntax: not valid UTF-8; nothing was changed",
                root.display()
            ))
        })?;
        Ok(Folder {
            root: root.to_path_buf(),
            hash,
            skills,
            shared_body,
            manifest,
        })
    }

    /// The source tables of the folder's `skiman.toml`, by skill name.
    fn sources(manifest: &Manifest) -> BTreeMap<String, SourceRecord> {
        manifest
            .skills()
            .into_keys()
            .filter_map(|name| manifest.source(&name).map(|s| (name, s)))
            .collect()
    }
}

/// The folder's `skiman.toml` with the canon's configuration merged in
/// by key, since an import brings skills and instructions only: the
/// canon's `[tools]` when the folder has none, each canon `[projects.<id>]`
/// and `[linked.<id>]` whose id the folder does not define (it defines no
/// `[linked]`: [`refuse_linked_tables`]), each canon `[[secrets.allow]]`
/// entry for a path below `projects/` the folder does not hold already,
/// and for each skill the folder `holds` but gives no `[skills.<name>]`
/// table, the canon's `summary`, `origin` and `installed` of it (its
/// `enabled` and `source` stay the folder's). Tables written inline count
/// on both sides. The folder's own text keeps its bytes, except an inline
/// `projects`, `secrets` or `skills` table that takes canon entries, which
/// becomes a standard table.
fn merge_canon_config(
    folder: &str,
    canon: &str,
    holds: impl Fn(&str) -> bool,
) -> Result<String, Failure> {
    let Ok(mut doc) = folder.parse::<DocumentMut>() else {
        // The rules report the syntax error with its line.
        return Ok(folder.to_owned());
    };
    let Ok(canon) = canon.parse::<DocumentMut>() else {
        return Ok(folder.to_owned());
    };
    let mut merged = false;
    if !doc.contains_key("tools")
        && let Some(mut tools) = canon.get("tools").cloned()
    {
        if let Some(table) = tools.as_table_mut() {
            table.set_position(Some(-1));
        }
        doc.insert("tools", tools);
        merged = true;
    }
    for key in ["projects", "linked"] {
        let Some(tables) = canon.get(key).and_then(Item::as_table_like) else {
            continue;
        };
        for (id, table) in tables.iter() {
            if doc.get(key).and_then(|kept| kept.get(id)).is_none()
                && let Ok(table) = table.clone().into_table()
                && let Some(into) = manifest::subtable(&mut doc, key)
            {
                into.insert(id, Item::Table(appended(table)));
                merged = true;
            }
        }
    }
    let skills = canon.get("skills").and_then(Item::as_table_like);
    for (name, table) in skills.into_iter().flat_map(|t| t.iter()) {
        if holds(name)
            && doc.get("skills").and_then(|kept| kept.get(name)).is_none()
            && let Ok(mut table) = table.clone().into_table()
        {
            table.retain(|key, _| ["summary", "origin", "installed"].contains(&key));
            if !table.is_empty()
                && let Some(into) = manifest::subtable(&mut doc, "skills")
            {
                into.insert(name, Item::Table(appended(table)));
                merged = true;
            }
        }
    }
    let mirror = format!("{MIRROR_DIR}/");
    let allows = canon
        .get("secrets")
        .and_then(|secrets| secrets.get("allow"))
        .and_then(Item::as_array_of_tables);
    let in_projects = |allow: &&Table| {
        allow
            .get("path")
            .and_then(Item::as_str)
            .is_some_and(|path| path.starts_with(&mirror))
    };
    let same = |a: &Table, b: &Table| {
        ["path", "rule", "fingerprint"]
            .iter()
            .all(|key| a.get(key).and_then(Item::as_str) == b.get(key).and_then(Item::as_str))
    };
    for allow in allows
        .into_iter()
        .flat_map(ArrayOfTables::iter)
        .filter(in_projects)
    {
        let kept = doc
            .get("secrets")
            .and_then(|secrets| secrets.get("allow"))
            .and_then(Item::as_array_of_tables);
        if !kept.is_some_and(|kept| kept.iter().any(|kept| same(kept, allow)))
            && let Some(secrets) = manifest::subtable(&mut doc, "secrets")
            && let Some(into) = secrets
                .entry("allow")
                .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
                .as_array_of_tables_mut()
        {
            into.push(appended(allow.clone()));
            merged = true;
        }
    }
    Ok(if merged {
        doc.to_string()
    } else {
        folder.to_owned()
    })
}

/// `table` from another document, placed after the tables it joins instead
/// of at its position there.
fn appended(mut table: Table) -> Table {
    table.set_position(None);
    table
}

/// The folder as the canon would hold it, with `manifest` as its `skiman.toml`.
fn folder_tree<'a>(working_tree: &'a WorkingTree, manifest: &str) -> Overlay<'a> {
    Overlay::new(
        working_tree,
        vec![(rules::MANIFEST.to_owned(), manifest.as_bytes().to_vec())],
    )
}

/// Phase 1: every rule except Consistency on the folder.
fn check_folder(content: &Folder, manifest: &str) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(&content.root);
    let tree = folder_tree(&working_tree, manifest);
    let mut scope = scope_all(&tree)?;
    scope.consistency.clear();
    refuse_violations(&content.root, &tree, &scope, "")
}

fn scope_all(tree: &dyn Tree) -> Result<Scope, Failure> {
    Scope::all(tree)
        .map_err(|e| Failure::operational(format!("cannot read the import folder: {e}")))
}

/// Refuses (exit 1) when anything in `scope` breaks a rule, naming file and
/// line, with the ready-to-paste secret exemptions.
fn refuse_violations(
    root: &Path,
    tree: &dyn Tree,
    scope: &Scope,
    note: &str,
) -> Result<(), Failure> {
    let violations = rules::check(tree, scope)
        .map_err(|e| Failure::operational(format!("cannot read {}: {e}", root.display())))?;
    if violations.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = violations.iter().map(|v| format!("  {v}")).collect();
    let mut message = format!(
        "refusing to import {}: it breaks the rules{note}; nothing was changed:\n{}",
        root.display(),
        lines.join("\n")
    );
    if let Some(hint) = rules::allow_hint(&violations) {
        message.push_str("\n\n");
        message.push_str(&hint);
    }
    Err(Failure::refused(message))
}

/// One third-party skill's upstream at its declared ref, fetched now or before.
struct Third {
    name: String,
    source: SourceRecord,
    snapshot: Snapshot,
    entry: LockEntry,
    /// The kind of `source.ref`: derived when fetched, else the canon's.
    ref_kind: Option<&'static str>,
    /// `at <commit> (tag v1)`, `(local folder)`: after the source in the preview.
    version: String,
}

/// Fetches every declared source at its declared `ref` (the default branch
/// when there is none) and reads its snapshot; path safety runs on each. A
/// source the canon already holds ([`fetched_before`]) is not fetched
/// again.
fn fetch_sources(
    paths: &Paths,
    repo: &Canon,
    content: &Folder,
    manifest_text: &str,
    prompts: Prompts,
) -> Result<Vec<Third>, Failure> {
    let manifest = Manifest::parse(manifest_text)
        .map_err(|e| Failure::refused(format!("skiman.toml: {e}")))?;
    // Read before anything is fetched, so a broken lock is reported first.
    let lock = Lockfile::load(repo)?;
    let mut thirds = Vec::new();
    for (name, source) in Folder::sources(&manifest) {
        let refuse = |why: &str| {
            Failure::refused(format!(
                "refusing to import {}: skiman.toml: [skills.{name}.source]: {why}; nothing was changed",
                content.root.display()
            ))
        };
        let url = &source.url;
        let kind = check_url(url).expect("check_folder refused an unsupported `url`");
        if let Some(third) = fetched_before(paths, (repo, &lock), &name, &source, prompts) {
            thirds.push(third);
            continue;
        }
        let upstream = if kind == UrlKind::Local {
            if source.git_ref.is_some() {
                return Err(refuse("a local folder has no versions; drop `ref`"));
            }
            let dir = upstream::local_root(paths, url).map_err(|f| refuse(&f.message))?;
            Upstream::Local { dir }
        } else {
            Upstream::at_ref(Cache::open(paths, url, prompts)?, source.git_ref.as_deref())?
        };
        let snapshot = upstream.snapshot(&source.path)?;
        thirds.push(Third {
            entry: upstream.lock_entry(&snapshot),
            ref_kind: upstream.resolved().map(|r| r.kind.as_str()),
            version: upstream.version(),
            name,
            source,
            snapshot,
        });
    }
    Ok(thirds)
}

/// The third-party skill `name` as the canon holds it, when an earlier
/// run fetched it from a git upstream as `source` declares it: the repo's
/// source table names the same `url`, `path` and `ref`, and its entry in `lock`
/// has that `ref` and a commit. Its snapshot is `upstream/<name>` when that
/// matches the entry, else the entry's commit read from the cache (fetched
/// when the cache lacks it), for a run stopped before it wrote the
/// snapshot. Fetching at the `ref` again would move a branch or tag that
/// moved since, and needs the network.
fn fetched_before(
    paths: &Paths,
    (repo, lock): (&Canon, &Lockfile),
    name: &str,
    source: &SourceRecord,
    prompts: Prompts,
) -> Option<Third> {
    let manifest = Manifest::load(repo).ok()?;
    let recorded = manifest.source(name)?;
    let entry = lock.entry(name)?;
    // A local folder has no commit: it is read again as it is now.
    let commit = entry.commit.clone()?;
    let same = (&recorded.url, &recorded.path, &recorded.git_ref)
        == (&source.url, &source.path, &source.git_ref)
        && entry.git_ref == source.git_ref;
    if !same {
        return None;
    }
    let locked = upstream::locked_snapshot(paths, prompts, repo, name, source, &entry);
    let snapshot = locked.ok().flatten()?;
    Some(Third {
        name: name.to_owned(),
        source: source.clone(),
        version: format!("at {commit} (as the canon holds it)"),
        ref_kind: recorded.ref_kind,
        snapshot,
        entry,
    })
}

/// `manifest` with each source's `ref_kind` set to its [`Third`]'s, or removed
/// when the source has no `ref`: a `ref_kind` the folder holds is never kept.
fn with_ref_kinds(manifest: String, thirds: &[Third]) -> Result<String, Failure> {
    thirds.iter().try_fold(manifest, |text, t| {
        manifest::with_source_ref(&text, &t.name, t.source.git_ref.as_deref(), t.ref_kind)
            .map_err(|e| Failure::refused(format!("skiman.toml: {e}")))
    })
}

/// The `skiman.lock` text with one entry per third-party skill.
fn lock_text(thirds: &[Third]) -> Result<String, Failure> {
    let mut text = format!("version = {}\n", 1);
    for third in thirds {
        text = lockfile::with_entry(&text, &third.name, &third.entry)
            .map_err(|e| Failure::operational(format!("skiman.lock: {e}")))?;
    }
    Ok(text)
}

/// Every file of `upstream/<name>` of every third-party skill: `(path, executable, content)`.
fn upstream_files(thirds: &[Third]) -> Vec<(String, bool, Vec<u8>)> {
    thirds
        .iter()
        .flat_map(|third| {
            third.snapshot.files.iter().map(|f| {
                (
                    format!("upstream/{}/{}", third.name, f.path),
                    f.executable,
                    f.content.clone(),
                )
            })
        })
        .collect()
}

/// Phase 2: every rule, Consistency and the secret rule included, on the
/// complete result: the folder plus `upstream/` and `skiman.lock`.
fn check_result(
    content: &Folder,
    manifest: &str,
    lock: &str,
    thirds: &[Third],
) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(&content.root);
    let files = upstream_files(thirds);
    let executable = files
        .iter()
        .filter(|(_, x, _)| *x)
        .map(|(p, _, _)| p.clone())
        .collect();
    let mut overlay: Vec<(String, Vec<u8>)> = files.into_iter().map(|(p, _, c)| (p, c)).collect();
    overlay.push((rules::MANIFEST.to_owned(), manifest.as_bytes().to_vec()));
    overlay.push((rules::LOCKFILE.to_owned(), lock.as_bytes().to_vec()));
    let tree = Overlay::new(&working_tree, overlay).with_executable(executable);
    let scope = scope_all(&tree)?;
    refuse_violations(
        &content.root,
        &tree,
        &scope,
        " once its upstreams are fetched (upstream/ and skiman.lock are built by skiman)",
    )
}

/// Refuses (exit 1) an import folder whose `skiman.toml` has `[linked]`
/// tables: an import brings skills and instructions only, and the canon's
/// own registrations are kept. Names the command that registers each.
fn refuse_linked_tables(content: &Folder) -> Result<(), Failure> {
    let Ok(manifest) = Manifest::parse(&content.manifest) else {
        // The rules report the syntax error with its line.
        return Ok(());
    };
    let tables = manifest.linked();
    if tables.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = tables
        .iter()
        .map(|entry| match &entry.path {
            Ok(path) => format!("  skiman linked add {path} --id {}", entry.id),
            Err(malformed) => format!("  [linked.{}]: {}", entry.id, malformed.why),
        })
        .collect();
    Err(Failure::refused(format!(
        "refusing to import {}: its skiman.toml has [linked] tables, but an import brings skills and instructions only; remove them from the import folder, and once the import is done register each folder with skiman linked add; nothing was changed:\n{}",
        content.root.display(),
        lines.join("\n")
    )))
}

/// The canon's linked folders on this machine, as registered in `manifest`
/// (the canon's, which the import keeps). Refuses (exit 1) while one cannot
/// be read, since what leads into it is unknown; when a skill folder or a
/// group folder of the import folder uses a linked skill's name; and when
/// a place the closing sync writes ([`sync::write_targets`], its cleanup of
/// tools that are not enabled included) is one sync's guard refuses
/// ([`sync::refusal`]): in a linked folder (spec "Write targets never reach
/// into a linked folder") or in the canon.
fn linked_folders(
    paths: &Paths,
    repo: &Canon,
    manifest: &Manifest,
    content: &Folder,
) -> Result<Linked, Failure> {
    let linked = Linked::read(paths.home(), repo, manifest);
    let unreadable = linked.folders.iter().find_map(|f| match &f.state {
        State::Unreadable(why) => Some((f, why)),
        _ => None,
    });
    if let Some((folder, why)) = unreadable {
        return Err(Failure::refused(format!(
            "refusing to import {}: linked folder `{}` cannot be read now ({why}), so skiman cannot tell what in the tool directories leads into it; nothing was changed, run the import again once it can be read",
            content.root.display(),
            folder.id
        )));
    }
    // Every folder the import writes under `skills/`, by path, with the name
    // it takes: each skill folder, and each group folder above one.
    let mut folders = BTreeMap::new();
    for (name, skill) in &content.skills {
        folders.insert(skill.path.as_str(), name.as_str());
        let mut path = skill.path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/')
            && let Some((_, group)) = parent.rsplit_once('/')
        {
            folders.insert(parent, group);
            path = parent;
        }
    }
    let lines: Vec<String> = folders
        .iter()
        .filter_map(|(path, name)| {
            let (it, _) = linked.described(name)?;
            Some(format!("  {path}: it is {it}"))
        })
        .collect();
    if !lines.is_empty() {
        return Err(Failure::refused(format!(
            "refusing to import {}: these skill folders and group folders use the name of a linked skill, which skiman never replaces; rename them in the import folder, nothing was changed:\n{}",
            content.root.display(),
            lines.join("\n")
        )));
    }
    // Every place the closing sync writes, its cleanup included: the
    // takeover moves nothing from where sync's guard writes nothing.
    let lines: Vec<String> = sync::write_targets(paths.home(), manifest)
        .iter()
        .filter_map(|t| {
            let file = t.place == Place::Instructions;
            sync::refusal((repo, &linked), (&t.path, &t.lands, file))
        })
        .map(|why| format!("  {why}"))
        .collect();
    if !lines.is_empty() {
        return Err(Failure::refused(format!(
            "refusing to import {}: it would write into a linked folder or the canon; nothing was changed:\n{}",
            content.root.display(),
            lines.join("\n")
        )));
    }
    Ok(linked)
}

/// The import folder's content, checked, plus what skiman built.
struct Candidate<'a> {
    content: &'a Folder,
    /// The `skiman.toml` text the import writes.
    manifest: String,
    /// The `skiman.lock` text the import writes.
    lock: String,
    thirds: Vec<Third>,
}

/// A tool-directory entry or instruction file the import moves to the backup.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Move {
    /// As the preview and the `moved` lines show it: in the tool directory
    /// as the tool names it, or the instruction location.
    path: PathBuf,
    /// Where it is: the realpath of its tool directory plus its name (for an
    /// instruction location, `path`). The move order compares this one, since
    /// what a link resolves through is a real path, and the move takes it.
    real: PathBuf,
    /// Where it goes, relative to the backup folder.
    backup: PathBuf,
    /// What sync links in its place; `None` when nothing does (a foreign
    /// skill the import folder does not hold).
    link: Option<PathBuf>,
    /// The [`resolution`] of `real`, which decides the move order.
    resolution: Vec<PathBuf>,
}

/// What happens to the tool directories and instruction locations. Worked
/// out again under the canon lock: a plan that differs from the previewed
/// one refuses the run.
#[derive(Debug, Default, PartialEq, Eq)]
struct Plan {
    /// Entries and instruction files moved to the backup, in order.
    moves: Vec<Move>,
    /// Managed-elsewhere entries: left alone.
    skipped: Vec<PathBuf>,
    /// Non-skill entries at a name the import distributes: left alone, so
    /// that skill is not distributed there. `(path, skill name)`.
    staying: Vec<(PathBuf, String)>,
    /// Entries that are, hold or lie in a linked folder or a target of one
    /// of its child links: never moved (spec "Nothing is moved out of a
    /// linked folder"). `(path, linked folder id)`.
    linked: Vec<(PathBuf, String)>,
}

impl Plan {
    /// Classifies every entry of every installed tool's tool directory (tool
    /// directories resolving to one real directory once) and every
    /// instruction location, as sync would after the import. Refuses (exit 1)
    /// what [`linked_folders`] refuses, and an installed tool's directory
    /// whose [`resolution`] passes through an entry that moves.
    fn make(
        paths: &Paths,
        repo: &Canon,
        manifest: &Manifest,
        content: &Folder,
    ) -> Result<Plan, Failure> {
        let home = paths.home();
        let linked = linked_folders(paths, repo, manifest, content)?;
        let entries = manifest.skills();
        let mut wanted: BTreeMap<String, PathBuf> = content
            .skills
            .iter()
            .filter(|(name, _)| entries.get(*name).is_none_or(|e| e.enabled))
            .map(|(name, skill)| (name.clone(), repo.root().join(&skill.path)))
            .collect();
        wanted.insert(skiman_manual::NAME.to_owned(), repo.manual_dir());
        // No import skill uses a linked skill's name ([`linked_folders`]).
        let layout = Layout::read(&WorkingTree::new(&content.root)).map_err(|e| {
            Failure::operational(format!("cannot read {}: {e}", content.root.display()))
        })?;
        let kept = sync::kept_names(paths, repo, manifest, &linked);
        wanted.extend(linked.names(&layout, &kept, |_| true).0);
        let ownership = Ownership::new(paths, repo, None);
        let io = |path: &Path, e: std::io::Error| {
            Failure::operational(format!("cannot read {}: {e}", path.display()))
        };
        let mut plan = Plan::default();
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();
        for tool in sync::installed_tools(home, manifest) {
            let dir = tool.tool_dir(home);
            let real = match dir.canonicalize() {
                Ok(real) => real,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io(&dir, e)),
            };
            if !seen.insert(real.clone()) {
                continue;
            }
            let mut names = Vec::new();
            for entry in std::fs::read_dir(&dir).map_err(|e| io(&dir, e))? {
                names.push(entry.map_err(|e| io(&dir, e))?.file_name());
            }
            names.sort();
            for name in names {
                let path = dir.join(&name);
                let meta = std::fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
                let text = name.to_string_lossy().into_owned();
                match ownership.classify(&real, &name, &meta) {
                    Entry::Owned(_) => {}
                    Entry::ManagedElsewhere => plan.skipped.push(path),
                    // Moving a moved one again finishes the move.
                    Entry::Foreign | Entry::Moved(_) => {
                        let real = real.join(&name);
                        if let Some(folder) = linked.reaches_into(&real) {
                            plan.linked.push((path, folder.id.clone()));
                            continue;
                        }
                        entries.push(Move {
                            resolution: resolution(&real),
                            path,
                            real,
                            backup: PathBuf::from(tool.id).join(&name),
                            link: wanted.get(&text).cloned(),
                        });
                    }
                    Entry::NotASkill if wanted.contains_key(&text) => {
                        plan.staying.push((path, text));
                    }
                    Entry::NotASkill => {}
                }
            }
        }
        let order = move_order(&entries).ok_or_else(|| {
            Failure::operational(
                "cannot find an order to move the tool-directory entries in: each one left resolves through another; nothing was changed",
            )
        })?;
        plan.moves
            .extend(order.into_iter().map(|i| entries[i].clone()));
        // A tool directory resolved through an entry that moves would lead
        // nowhere once it moved, and sync could not write there.
        let lines: Vec<String> = sync::installed_tools(home, manifest)
            .map(|tool| tool.tool_dir(home))
            .filter_map(|dir| {
                let hops = resolution(&dir);
                let entry = plan
                    .moves
                    .iter()
                    .find(|m| hops.iter().any(|hop| hop.starts_with(&m.real)))?;
                Some(format!(
                    "  {}: through {}",
                    dir.display(),
                    entry.path.display()
                ))
            })
            .collect();
        if !lines.is_empty() {
            return Err(Failure::refused(format!(
                "refusing to import {}: these tool directories lead through an entry the import moves to the backup, after which they would lead nowhere; replace each with a real folder, nothing was changed:\n{}",
                content.root.display(),
                lines.join("\n")
            )));
        }
        let mut seen = BTreeSet::new();
        for tool in sync::installed_tools(home, manifest) {
            let path = home.join(tool.instruction_file);
            let Ok(location) = sync::instruction_location(&path) else {
                continue;
            };
            if !seen.insert(location.clone()) {
                continue;
            }
            if let Some(folder) = linked.reaches_into(&location) {
                plan.linked.push((location, folder.id.clone()));
                continue;
            }
            let meta = match std::fs::symlink_metadata(&location) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io(&location, e)),
            };
            if ownership.owned_instruction(&location, &meta).is_none() {
                let file_name = location.file_name().expect("location has a file name");
                plan.moves.push(Move {
                    backup: PathBuf::from(tool.id).join(file_name),
                    // Only tool-directory entries are ordered by what they pass through.
                    resolution: Vec::new(),
                    path: location.clone(),
                    real: location,
                    link: Some(repo.shared_body()),
                });
            }
        }
        Ok(plan)
    }
}

/// The paths `path` passes through when it is resolved like realpath,
/// component by component from the root: each link met, at its path with
/// the folders above it already resolved, before its target is resolved in
/// turn; the folder each `..` leaves, before going up; and where the path
/// ends. A path with no link and no `..` on the way gives just itself.
/// After 40 links (links followed, not paths met: a `..` adds a path too)
/// the rest is taken as it is; canonicalize fails before that, so such an
/// entry is never a skill.
fn resolution(path: &Path) -> Vec<PathBuf> {
    let mut left: Vec<PathBuf> = path
        .components()
        .rev()
        .map(|c| c.as_os_str().into())
        .collect();
    let mut at = PathBuf::new();
    let mut hops = Vec::new();
    let mut links = 0;
    while let Some(part) = left.pop() {
        if part.as_os_str() == ".." {
            hops.push(at.clone());
            at.pop();
            continue;
        }
        at.push(&part);
        if links < 40
            && let Ok(target) = std::fs::read_link(&at)
        {
            links += 1;
            hops.push(at.clone());
            at.pop();
            left.extend(target.components().rev().map(|c| c.as_os_str().into()));
        }
    }
    hops.push(at);
    hops
}

/// The order to move the tool-directory entries in, as indexes into
/// `entries`. An entry passes through another when a path of its
/// [`resolution`] equals or is below the other's real path; it moves first,
/// so a run that stops between the two still finds it a skill when it is
/// run again. Each step takes the first entry left, in the given order (by
/// tool, then name), that no entry left passes through: a topological
/// order. Reads no file system. Entries that resolve always leave a next
/// one, since what each passes through is finite and has no cycle; `None`
/// when none is left to take, which only a file system changed between the
/// classification and the resolution can cause.
fn move_order(entries: &[Move]) -> Option<Vec<usize>> {
    let passes = |e: usize, m: usize| {
        e != m
            && entries[e]
                .resolution
                .iter()
                .any(|hop| hop.starts_with(&entries[m].real))
    };
    let mut left: Vec<usize> = (0..entries.len()).collect();
    let mut order = Vec::new();
    while !left.is_empty() {
        let next = left
            .iter()
            .position(|&m| !left.iter().any(|&e| passes(e, m)))?;
        order.push(left.remove(next));
    }
    Some(order)
}

/// Refuses (exit 1) when an original that would be moved to the backup is
/// or contains a special file (FIFO, socket, device).
fn refuse_special_files(plan: &Plan) -> Result<(), Failure> {
    let mut found = Vec::new();
    for entry in &plan.moves {
        found.extend(backup::special_files(&entry.path).map_err(|e| {
            Failure::operational(format!("cannot read {}: {e}", entry.path.display()))
        })?);
    }
    found.sort();
    if found.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = found.iter().map(|p| format!("  {}", p.display())).collect();
    Err(Failure::refused(format!(
        "refusing to import: these special files (FIFO, socket or device) are in entries the import would move to the backup; move them away by hand, nothing was changed:\n{}",
        lines.join("\n")
    )))
}

/// Refuses (exit 1) when the local folder a third-party skill is read from
/// ([`upstream::local_root`] joined with its `path`) would be gone after
/// the takeover, so that neither `update` nor this import run again could
/// read it: its path, resolved like [`resolution`], passes through an entry
/// that moves (a link included), or it is in, or holds, the real folder of
/// one. Being in one compares the source's canonical path, so it also holds
/// where the written path is spelt otherwise than on disk, such as in
/// another case. Moving a link that leads to a source elsewhere leaves the
/// source where it is; a source whose path passes through that link is
/// refused all the same. Entries compare by their real path, byte for byte.
/// The sources come from `manifest`, the `skiman.toml` the takeover plan
/// follows; a source folder that does not exist is skipped.
fn refuse_moving_sources(
    paths: &Paths,
    content: &Folder,
    manifest: &Manifest,
    plan: &Plan,
) -> Result<(), Failure> {
    let mut lines = Vec::new();
    for (name, record) in Folder::sources(manifest) {
        if !matches!(check_url(&record.url), Ok(UrlKind::Local)) {
            continue;
        }
        let Ok(root) = upstream::local_root(paths, &record.url) else {
            continue;
        };
        let written = upstream::join(&root, &record.path);
        let Ok(source) = written.canonicalize() else {
            continue;
        };
        let hops = resolution(&written);
        for entry in &plan.moves {
            let folder = std::fs::symlink_metadata(&entry.real).is_ok_and(|meta| meta.is_dir());
            if source.starts_with(&entry.real)
                || hops.iter().any(|hop| hop.starts_with(&entry.real))
                || (folder && entry.real.starts_with(&source))
            {
                let path = if record.path.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", record.path)
                };
                lines.push(format!(
                    "  skill {name}: source {}{path}, entry {}",
                    record.url,
                    entry.path.display()
                ));
            }
        }
    }
    if lines.is_empty() {
        return Ok(());
    }
    Err(Failure::refused(format!(
        "refusing to import {}: the local folder a third-party skill is read from is in, holds or is reached through an entry the import moves to the backup, after which the skill cannot be read from it again; in the import folder's skiman.toml, point the source at a copy of that upstream outside the tool directories, or drop the skill's source table to import it as an own skill; nothing was changed:\n{}",
        content.root.display(),
        lines.join("\n")
    )))
}

/// Where the import folder's `skiman.toml` (`folder`, merged with the canon's
/// as the import would write it) differs from the canon's
/// (`repo`) in what is distributed: `[tools] enabled` as a set, `enabled` of
/// each skill, and the `ref` of each source. Shown when the canon
/// already holds the import, which keeps the repo's.
fn config_differences(folder: &Manifest, repo: &Manifest, content: &Folder) -> Vec<String> {
    let mut differs = Vec::new();
    let tools = |m: &Manifest| m.enabled_tools().into_iter().collect::<BTreeSet<_>>();
    if tools(folder) != tools(repo) {
        differs.push("[tools] enabled".to_owned());
    }
    let (theirs, ours) = (folder.skills(), repo.skills());
    let enabled = |entries: &BTreeMap<String, SkillEntry>, name: &str| {
        entries.get(name).is_none_or(|entry| entry.enabled)
    };
    for name in content.skills.keys() {
        if enabled(&theirs, name) != enabled(&ours, name) {
            differs.push(format!("[skills.{name}] enabled"));
        }
    }
    let ours = Folder::sources(repo);
    for (name, source) in Folder::sources(folder) {
        if ours.get(&name).map(|s| &s.git_ref) != Some(&source.git_ref) {
            differs.push(format!("[skills.{name}.source] ref"));
        }
    }
    differs
}

/// The files of skill `name` that differ from its upstream snapshot.
fn local_modifications(skill: &Snapshot, upstream: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    for file in &skill.files {
        match upstream.file(&file.path) {
            None => out.push(format!("added {}", file.path)),
            Some(theirs) if theirs != file => out.push(format!("modified {}", file.path)),
            Some(_) => {}
        }
    }
    for file in &upstream.files {
        if skill.file(&file.path).is_none() {
            out.push(format!("deleted {}", file.path));
        }
    }
    out.sort_by(|a, b| {
        let path = |s: &str| {
            s.split_once(' ')
                .map(|(_, p)| p.to_owned())
                .unwrap_or_default()
        };
        path(a).cmp(&path(b))
    });
    out
}

/// The preview lines for what the import writes to the canon.
fn print_writes(repo: &Canon, candidate: &Candidate, change: &Change, todo: &[Held]) {
    let held = |rel: &str| {
        let i = change.pieces.iter().position(|(r, _)| r == rel);
        i.map(|i| &todo[i])
    };
    for (name, skill) in &candidate.content.skills {
        let verb = match held(&skill.path) {
            Some(Held::Same) => "keep",
            _ => "add",
        };
        match candidate.thirds.iter().find(|t| &t.name == name) {
            None => out!("  {verb} skill {name} (own)"),
            Some(third) => {
                let changes = local_modifications(&skill.snapshot, &third.snapshot);
                let summary = if changes.is_empty() {
                    "; no file differs from upstream"
                } else {
                    "; files that differ from upstream:"
                };
                let (url, path) = (&third.source.url, &third.source.path);
                let path = if path.is_empty() {
                    String::new()
                } else {
                    format!(" ({path})")
                };
                let version = &third.version;
                out!("  {verb} skill {name} (third-party, from {url}{path} {version}){summary}");
                for change in changes {
                    out!("      {change}");
                }
                print_not_installed(&third.snapshot, "      ");
            }
        }
    }
    for ((rel, _), held) in change.pieces.iter().zip(todo) {
        if matches!(held, Held::Other(_)) {
            out!(
                "  move {} to the backup, then write the import's",
                repo.root().join(rel).display()
            );
        }
    }
}

/// The preview lines for the tool directories and instruction locations.
fn print_plan(repo: &Canon, plan: &Plan, paths: &Paths) {
    for location in &plan.moves {
        match &location.link {
            Some(target) if target == &repo.shared_body() => out!(
                "  move {} to the backup, then link it to the shared body",
                location.path.display()
            ),
            Some(_) => out!(
                "  move {} to the backup, then link it to skill {}",
                location.path.display(),
                tree::basename(&location.backup.to_string_lossy())
            ),
            None => out!(
                "  move {} to the backup (no imported skill of that name is distributed)",
                location.path.display()
            ),
        }
    }
    for path in &plan.skipped {
        out!("  skip {}: managed elsewhere", path.display());
    }
    for (path, name) in &plan.staying {
        out!(
            "  keep {}: it is not a skill, so skill {name} is not linked there",
            path.display()
        );
    }
    for (path, id) in &plan.linked {
        out!(
            "  keep {}: it holds or lies in linked folder `{id}`, whose content skiman never moves",
            path.display()
        );
    }
    out!(
        "originals go to a new folder in {}",
        paths.state_dir().join("backups").display()
    );
}

impl Candidate<'_> {
    /// Every piece, in the order the import writes them: `skiman.toml`,
    /// `skiman.lock`, `upstream/`, the shared body, `skills/`. [`progress`]
    /// and [`fetched_before`] depend on that order:
    /// - `skiman.toml` comes first, so a canon that holds any other
    ///   piece as the import writes it holds the import's `skiman.toml` too,
    ///   and a shared body that is the import's says so.
    /// - The source tables and lock entries come before the snapshots, so a
    ///   run stopped after them reads each snapshot again at the commit the
    ///   lock entry has, instead of fetching a branch or tag that moved.
    /// - The skills come last: written before a shared body that held
    ///   something else, a run stopped between the two would be refused when
    ///   run again.
    fn change(&self) -> Change {
        let file = |rel: &str, bytes: &[u8]| (rel.to_owned(), Want::File(bytes.to_vec()));
        let folder = |rel: String, snapshot: &Snapshot| (rel, Want::Folder(snapshot.files.clone()));
        let mut pieces = vec![
            file(rules::MANIFEST, self.manifest.as_bytes()),
            file(rules::LOCKFILE, self.lock.as_bytes()),
        ];
        let thirds = self.thirds.iter();
        pieces.extend(thirds.map(|t| folder(format!("upstream/{}", t.name), &t.snapshot)));
        pieces.push(file(SHARED_BODY, &self.content.shared_body));
        let skills = self.content.skills.values();
        pieces.extend(skills.map(|skill| folder(skill.path.clone(), &skill.snapshot)));
        Change {
            pieces,
            command: "import",
            save_files: true,
        }
    }
}

/// The names in `dir` that do not start with `.`, sorted; none when `dir` is missing.
fn names_in(dir: &Path) -> Result<Vec<String>, Failure> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(Failure::operational(format!(
                "cannot read {}: {e}",
                dir.display()
            )));
        }
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    Ok(names)
}

/// Refuses (exit 1) when `skiman.toml`, `skiman.lock` or the shared body of
/// the canon is there but is not a regular file.
fn refuse_irregular_files(repo: &Canon) -> Result<(), Failure> {
    let mut lines = Vec::new();
    for rel in CANON_FILES {
        let path = repo.root().join(rel);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if !meta.is_file() => lines.push(format!("  {rel} is not a regular file")),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(Failure::operational(format!(
                    "cannot read {}: {e}",
                    path.display()
                )));
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        return Ok(());
    }
    Err(Failure::refused(format!(
        "the canon {} holds a link, folder or special file where import needs a regular file; replace each with a regular file or move it away, nothing was changed:\n{}",
        repo.root().display(),
        lines.join("\n")
    )))
}

/// How far the canon is with this import, decided before anything is fetched.
#[derive(Debug, PartialEq, Eq)]
enum Progress {
    /// The repo holds everything the import writes.
    Done,
    /// The repo holds everything but the import's shared body, and its own
    /// holds more than whitespace.
    OtherSharedBody,
    /// Anything else: the whole import is worked out again ([`canon_todo`]).
    NotDone,
}

/// Done when the repo's `skills/` holds exactly the folder's skills, each the
/// same; the repo's `skiman.toml` has exactly the folder's source tables, with
/// the same `url` and `path` (not `ref`, which `update --ref` changes after
/// an import); Consistency holds in the repo; and the shared bodies are the
/// same. Shared bodies of nothing but ASCII whitespace say the same, so a
/// missing or blank one in the repo is the same as a blank one in the
/// folder; a blank one in the repo holds nothing to keep against a folder's
/// that holds something. `repo_manifest` is the repo's `skiman.toml`; one
/// that does not parse is not done.
fn progress(
    repo: &Canon,
    content: &Folder,
    folder_manifest: &Manifest,
    repo_manifest: &str,
) -> Result<Progress, Failure> {
    let held: BTreeSet<String> = repo.layout()?.skills().iter().map(Placed::path).collect();
    if !held.iter().eq(content
        .skills
        .values()
        .map(|s| &s.path)
        .collect::<BTreeSet<_>>())
    {
        return Ok(Progress::NotDone);
    }
    for skill in content.skills.values() {
        let want = Want::Folder(skill.snapshot.files.clone());
        if change::held(repo.root(), &skill.path, &want)? != Held::Same {
            return Ok(Progress::NotDone);
        }
    }
    let Ok(repo_manifest) = Manifest::parse(repo_manifest) else {
        return Ok(Progress::NotDone);
    };
    let sources = |manifest: &Manifest| -> Vec<_> {
        Folder::sources(manifest)
            .into_iter()
            .map(|(name, s)| (name, s.url, s.path))
            .collect()
    };
    if sources(&repo_manifest) != sources(folder_manifest) {
        return Ok(Progress::NotDone);
    }
    let consistent = rules::consistent(&WorkingTree::new(repo.root()))
        .map_err(|e| Failure::operational(format!("cannot read {}: {e}", repo.root().display())))?;
    if !consistent {
        return Ok(Progress::NotDone);
    }
    let blank = |bytes: &[u8]| bytes.iter().all(u8::is_ascii_whitespace);
    Ok(
        match change::held(
            repo.root(),
            SHARED_BODY,
            &Want::File(content.shared_body.clone()),
        )? {
            Held::Same => Progress::Done,
            Held::Nothing if blank(&content.shared_body) => Progress::Done,
            Held::Other(old) if blank(&old) && blank(&content.shared_body) => Progress::Done,
            Held::Other(old) if !blank(&old) => Progress::OtherSharedBody,
            _ => Progress::NotDone,
        },
    )
}

/// The refusal for [`Progress::OtherSharedBody`]. The repo alone cannot tell
/// an import that ran (finished or stopped part-way) from one that never
/// did, so the way on depends on which it is.
fn other_shared_body(repo: &Canon, folder: &Path) -> Failure {
    let theirs = folder.join(SHARED_BODY).display().to_string();
    let ours = repo.shared_body().display().to_string();
    Failure::refused(format!(
        "refusing to import {}: the canon {} already holds everything this import writes except {SHARED_BODY}, which differs from the import folder's; nothing was changed.\n  \
         If this import has run on this canon before (it finished or stopped part-way), make the two files the same and run it again: it finishes what is left and writes nothing else to the canon, whose skiman.toml stays as it is.\n    \
         to keep the canon's shared body, copy {ours} over {theirs};\n    \
         to use the import folder's, copy {theirs} over {ours} (skiman keeps no copy of the file you overwrite).\n  \
         If it has never run on this canon, empty {ours} (put what it holds into the import folder's shared body first if you want to keep it) and run it again: it imports everything, the import folder's skiman.toml included.\n  \
         If you cannot tell which, do the first.",
        folder.display(),
        repo.root().display(),
    ))
}

/// What the canon holds where each piece of `change` goes. Refuses
/// (exit 1, nothing changed), naming each with its ways out, a
/// `skills/<name>` that differs from the folder's, anything in `skills/`,
/// `upstream/` or `skiman.lock` that the import does not write, and a piece
/// that is not a regular file or folder.
fn canon_todo(repo: &Canon, candidate: &Candidate, change: &Change) -> Result<Vec<Held>, Failure> {
    let todo = change.held(repo.root())?;
    let mut lines = Vec::new();
    for ((rel, want), held) in change.pieces.iter().zip(&todo) {
        match (held, want) {
            (Held::WrongType, Want::Folder(_)) => lines.push(format!(
                "{rel} is not a folder: replace it with one, or move it away"
            )),
            (Held::WrongType, _) => lines.push(format!(
                "{rel} is not a regular file: replace it with one, or move it away"
            )),
            (Held::Other(_), _) if rel.starts_with("skills/") => lines.push(format!(
                "{rel} differs from the import folder's {rel}: replace the import folder's with the canon's to keep it, or change the canon's back to the import folder's"
            )),
            _ => {}
        }
    }
    let third = |name: &str| candidate.thirds.iter().any(|t| t.name == name);
    let imported = |path: &str| candidate.content.skills.values().any(|s| s.path == path);
    for path in repo.layout()?.skills().iter().map(Placed::path) {
        if !imported(&path) && !tree::basename(&path).starts_with('.') {
            lines.push(format!(
                "{path} is not in the import folder: remove it from the canon, or put it in the import folder"
            ));
        }
    }
    for name in names_in(&repo.root().join("upstream"))? {
        if !third(&name) {
            lines.push(format!(
                "upstream/{name} is the snapshot of no third-party skill in the import folder: remove it from the canon, or put {name} in the import folder as a third-party skill"
            ));
        }
    }
    for name in Lockfile::load(repo)?.skill_names() {
        if !third(&name) {
            lines.push(format!(
                "skiman.lock has an entry for {name}, which is no third-party skill in the import folder: remove the entry from the canon's skiman.lock, or put {name} in the import folder as a third-party skill"
            ));
        }
    }
    if lines.is_empty() {
        return Ok(todo);
    }
    let lines: Vec<String> = lines.iter().map(|l| format!("  {l}")).collect();
    Err(Failure::refused(format!(
        "the canon {} holds content that is not this import's; nothing was changed:\n{}",
        repo.root().display(),
        lines.join("\n")
    )))
}

/// Moves each original of `plan` to the backup, `moved` once one is in. Sync
/// creates the owned link in its place. An original that is gone needs
/// nothing. Each is taken from where the plan checked it (`real`), never
/// through a link on its way that may have changed since.
fn take_over(plan: &Plan, backups: &mut Backups, moved: &mut bool) -> Result<(), Failure> {
    for entry in &plan.moves {
        let point = entry.backup.display();
        test_hook::at("import", &format!("back-up:{point}"));
        if std::fs::symlink_metadata(&entry.real).is_ok() {
            let dest = backups.move_in(&entry.real, &entry.backup).map_err(|e| {
                Failure::operational(format!(
                    "cannot move {} to the backup: {e}",
                    entry.path.display()
                ))
            })?;
            out!("moved {} to {}", entry.path.display(), dest.display());
            *moved = true;
        }
        test_hook::at("import", &format!("moved:{point}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Move, merge_canon_config, move_order, resolution};

    fn entry(path: &str, hops: &[&str]) -> Move {
        Move {
            path: path.into(),
            real: path.into(),
            backup: PathBuf::new(),
            link: None,
            resolution: hops.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn a_link_to_a_link_to_a_folder_moves_before_both_and_an_entry_nothing_passes_through_moves_first()
     {
        // gemini/notes -> agents/notes -> claude/notes
        let entries = [
            entry("/h/claude/notes", &["/h/claude/notes"]),
            entry("/h/claude/old", &["/h/claude/old"]),
            entry("/h/agents/notes", &["/h/agents/notes", "/h/claude/notes"]),
            entry(
                "/h/gemini/notes",
                &["/h/gemini/notes", "/h/agents/notes", "/h/claude/notes"],
            ),
        ];
        assert_eq!(move_order(&entries), Some(vec![1, 3, 2, 0]));
    }

    #[test]
    fn a_link_through_an_alias_moves_before_the_alias_and_the_alias_before_its_folder() {
        // agents/notes -> claude/alias -> claude/notes
        let entries = [
            entry("/h/claude/alias", &["/h/claude/alias", "/h/claude/notes"]),
            entry("/h/claude/notes", &["/h/claude/notes"]),
            entry(
                "/h/agents/notes",
                &["/h/agents/notes", "/h/claude/alias", "/h/claude/notes"],
            ),
        ];
        assert_eq!(move_order(&entries), Some(vec![2, 0, 1]));
    }

    #[test]
    fn a_link_into_a_folder_inside_another_entry_moves_before_that_entry() {
        let entries = [
            entry("/h/claude/notes", &["/h/claude/notes"]),
            entry("/h/agents/deep", &["/h/agents/deep", "/h/claude/notes/sub"]),
            entry(
                "/h/agents/outside",
                &["/h/agents/outside", "/h/elsewhere/x"],
            ),
        ];
        assert_eq!(move_order(&entries), Some(vec![1, 0, 2]));
    }

    #[test]
    fn a_link_into_a_folder_below_another_moving_link_moves_before_that_link() {
        // eee -> mmm/sub, mmm -> nnn -> ddd
        let entries = [
            entry("/h/c/ddd", &["/h/c/ddd"]),
            entry("/h/c/mmm", &["/h/c/mmm", "/h/c/nnn", "/h/c/ddd"]),
            entry("/h/c/nnn", &["/h/c/nnn", "/h/c/ddd"]),
            entry(
                "/h/x/eee",
                &["/h/x/eee", "/h/c/mmm", "/h/c/nnn", "/h/c/ddd/sub"],
            ),
        ];
        assert_eq!(move_order(&entries), Some(vec![3, 1, 2, 0]));
    }

    #[test]
    fn entries_nothing_passes_through_keep_their_order() {
        let entries = [
            entry("/h/c/bbb", &["/h/c/bbb"]),
            entry("/h/c/aaa", &["/h/c/aaa", "/h/elsewhere/aaa"]),
            entry("/h/x/ccc", &["/h/x/ccc"]),
        ];
        assert_eq!(move_order(&entries), Some(vec![0, 1, 2]));
    }

    #[test]
    fn entries_that_pass_through_each_other_have_no_order() {
        let entries = [
            entry("/h/c/aaa", &["/h/c/aaa", "/h/c/bbb/x"]),
            entry("/h/c/bbb", &["/h/c/bbb", "/h/c/aaa/y"]),
            entry("/h/c/ccc", &["/h/c/ccc"]),
        ];
        assert_eq!(move_order(&entries), None);
    }

    fn root() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        (temp, root)
    }

    #[test]
    fn a_link_whose_target_goes_into_a_folder_and_back_out_passes_through_that_folder() {
        let (_temp, root) = root();
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::create_dir_all(root.join("notes")).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(root.join("real/../notes"), &link).unwrap();
        let hops = resolution(&link);
        assert!(hops.contains(&root.join("real")), "{hops:?}");
        assert_eq!(hops.last(), Some(&root.join("notes")));
    }

    #[test]
    fn a_link_after_forty_one_dot_dots_in_a_target_is_still_among_the_paths_passed_through() {
        let (_temp, root) = root();
        std::fs::create_dir_all(root.join("pad")).unwrap();
        std::fs::create_dir_all(root.join("real/notes")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("bridge")).unwrap();
        let target = format!("{}/{}bridge/notes", root.display(), "pad/../".repeat(41));
        let link = root.join("link");
        std::os::unix::fs::symlink(Path::new(&target), &link).unwrap();
        let hops = resolution(&link);
        assert!(hops.contains(&root.join("bridge")), "{hops:?}");
        assert_eq!(hops.last(), Some(&root.join("real/notes")));
    }

    #[test]
    fn merging_the_canons_configuration_into_its_own_result_changes_no_byte() {
        let tools = "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n";
        let allow = "[[secrets.allow]]\npath = \"projects/proj/AGENTS.md\"\nrule = \"aws-access-key\"\nfingerprint = \"sha256:00\"\n";
        let proj = "[projects.proj]\npath = \"~/proj\"\n";
        // (the canon's skiman.toml, the import folder's)
        let cases = [
            (format!("{tools}\n{proj}\n{allow}"), tools.to_owned()),
            (
                format!("[tools]\nenabled = [\"claude-code\"]\n\n{proj}"),
                "[skills.notes]\nenabled = true\n".to_owned(),
            ),
            (
                format!("projects = {{ proj = {{ path = \"~/proj\" }} }}\n\n{tools}\n{allow}"),
                tools.to_owned(),
            ),
            (
                format!("{tools}\n[projects]\nproj = {{ path = \"~/proj\" }}\n\n{allow}"),
                tools.to_owned(),
            ),
            (
                format!("{tools}\n{proj}\n{allow}"),
                format!("projects = {{}}\n{tools}"),
            ),
            (
                format!("{tools}\n{proj}\n{allow}"),
                format!("projects = {{ other = {{ path = \"~/other\" }} }}\n{tools}"),
            ),
            (
                format!("{tools}\n{proj}\n{allow}"),
                format!("secrets = {{}}\n{tools}"),
            ),
            (
                format!(
                    "# top\n{tools}\n# about proj\n[projects.proj] # trailing\npath = \"~/proj\" # why\n\n# allow\n{allow}"
                ),
                format!("# folder\n{tools}# end\n"),
            ),
            (
                format!("{tools}\n{proj}\n{allow}"),
                format!("{tools}\n[projects.other]\npath = \"~/other\"\n\n{allow}"),
            ),
            (
                format!("{tools}\n[linked.company]\npath = \"~/work/company\"\n\n{proj}"),
                tools.to_owned(),
            ),
        ];
        for (canon, folder) in cases {
            let merged = merge_canon_config(&folder, &canon, |_| true).unwrap();
            assert_eq!(
                merge_canon_config(&folder, &merged, |_| true).unwrap(),
                merged,
                "canon:\n{canon}\nfolder:\n{folder}"
            );
        }
    }
}
