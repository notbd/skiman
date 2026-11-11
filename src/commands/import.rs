//! `skiman import <folder>`: bring an existing setup in from a prepared
//! import folder (ADR 0001), and `skiman import --guide`, the guide an agent
//! follows to prepare one (read-only).
//!
//! Order: no TTY and no `--yes` -> exit 2 -> config -> the skill repo is
//! initialised, and its `skiman.toml`, `skiman.lock` and shared body are
//! regular files -> phase 1 on the folder (layout, path safety, every rule
//! except Consistency) -> whether the skill repo already holds the whole
//! import ([`progress`]; if so, nothing is fetched or written and the run
//! goes on at the takeover plan) -> fetch every declared source -> phase 2
//! on the complete result (Consistency, secrets, everything) -> what the
//! skill repo holds against what the import writes ([`repo_todo`]) ->
//! takeover plan -> preview -> confirm -> repo lock, checks again (the
//! takeover plan included) -> tmp leftovers -> `upstream/`, `skiman.lock`,
//! `skiman.toml`, `instructions/shared.md`, `skills/` (each staged under
//! `.skiman/tmp/` and renamed into place) -> originals in tool directories
//! and instruction locations to the backup -> sync, which creates every
//! link. Never commits.
//!
//! There is no record of a run (ADR 0005). An import that stopped part-way
//! is finished by running the same command again: every run works out the
//! whole import again, keeps what is already in place and does the rest.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};
use unicode_normalization::UnicodeNormalization;

use crate::Exit;
use crate::backup::Backup;
use crate::commands::{confirm, has_terminal};
use crate::failure::{Failure, finish};
use crate::fetch::{Cache, RefKind};
use crate::fsutil;
use crate::git::Prompts;
use crate::import_guide;
use crate::lockfile::{self, LockEntry, Lockfile};
use crate::managing_skills;
use crate::manifest::{self, Manifest, SkillEntry, SourceRecord};
use crate::ownership::{Entry, Ownership};
use crate::path_safety::{self, Content};
use crate::paths::Paths;
use crate::project::item::MIRROR_DIR;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::snapshot::Snapshot;
use crate::source::{UrlKind, check_url};
use crate::staging::{self, Staging};
use crate::sync::{self, Output};
use crate::test_hook;
use crate::tree::{self, Kind, Overlay, Tree, WorkingTree};

pub fn guide() -> Exit {
    finish(
        std::io::stdout()
            .write_all(import_guide::text().as_bytes())
            .map(|()| Exit::Success)
            .map_err(|e| Failure::operational(format!("cannot print the guide: {e}"))),
    )
}

/// `folder` defaults to `<skill repo>/.skiman/import/`.
pub fn run(folder: Option<&Path>, yes: bool) -> Exit {
    finish(import(folder, yes))
}

const SHARED_BODY: &str = "instructions/shared.md";

fn import(folder: Option<&Path>, yes: bool) -> Result<Exit, Failure> {
    let terminal = has_terminal();
    if !terminal && !yes {
        return Err(Failure::usage(
            "missing flag without a terminal: --yes (import without asking for confirmation)",
        ));
    }
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let folder = folder.map_or_else(|| repo.default_import_dir(), Path::to_path_buf);
    let folder = folder.canonicalize().map_err(|e| {
        Failure::refused(format!(
            "the import folder {} is not reachable: {e}",
            folder.display()
        ))
    })?;
    refuse_irregular_files(&repo)?;
    let repo_manifest = std::fs::read_to_string(repo.manifest_file()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Failure::refused(format!(
                "{} is not an initialised skill repo (it has no skiman.toml); run `skiman init {}`",
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
    let prompts = if terminal {
        Prompts::Allowed
    } else {
        Prompts::Never
    };

    let content = Folder::read(&folder)?;
    let manifest_text = merge_repo_config(&content.manifest, &repo_manifest)?;
    check_folder(&content, &manifest_text)?;
    let folder_manifest = Manifest::parse(&manifest_text)
        .map_err(|e| Failure::refused(format!("skiman.toml: {e}")))?;
    // `None` when the skill repo already holds the whole import (an earlier
    // run wrote it): nothing is fetched or written, and the takeover follows
    // the repo's own skiman.toml.
    let (writes, manifest) = match progress(&repo, &content, &folder_manifest, &repo_manifest)? {
        Progress::Done => (
            None,
            Manifest::parse(&repo_manifest).expect("parsed by progress"),
        ),
        Progress::OtherSharedBody => return Err(other_shared_body(&repo, &folder)),
        Progress::NotDone => {
            let thirds = fetch_sources(&paths, &content, &manifest_text, prompts)?;
            let manifest_text = with_ref_kinds(&manifest_text, &thirds)?;
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
            let todo = repo_todo(&repo, &candidate)?;
            (Some((candidate, todo)), manifest)
        }
    };
    let plan = Plan::make(&paths, &repo, &manifest, &content)?;
    refuse_special_files(&plan)?;

    out!(
        "import {} into the skill repo {}:",
        content.root.display(),
        repo.root().display()
    );
    match &writes {
        Some((candidate, todo)) => print_writes(&repo, candidate, todo),
        None => {
            out!(
                "  the skill repo already holds everything this import writes: nothing is written to it, and its skiman.toml stays as it is"
            );
            let differs = config_differences(&folder_manifest, &manifest, &content);
            if !differs.is_empty() {
                out!(
                    "  the import folder's skiman.toml differs from the skill repo's in {}; the skill repo's are kept",
                    differs.join(", ")
                );
            }
        }
    }
    print_plan(&repo, &plan, &paths);
    if !yes && !confirm(&format!("Import {}? [y/N] ", folder.display()))? {
        return Err(Failure::refused("not imported; nothing was changed"));
    }

    let lock = RepoLock::wait(&repo)?;
    refuse_irregular_files(&repo)?;
    let changed = match &writes {
        Some((candidate, todo)) => repo_todo(&repo, candidate)? != *todo,
        // The takeover plan follows the repo's skiman.toml: any byte of it counts.
        None => {
            progress(&repo, &content, &folder_manifest, &repo_manifest)? != Progress::Done
                || std::fs::read(repo.manifest_file()).ok().as_deref()
                    != Some(repo_manifest.as_bytes())
        }
    };
    if changed {
        return Err(Failure::refused(
            "the skill repo changed after the preview; nothing was changed, run the import again",
        ));
    }
    if Folder::hash(&folder)? != content.hash {
        return Err(Failure::refused(format!(
            "the import folder {} changed after the preview; nothing was changed, run the import again",
            folder.display()
        )));
    }
    // What moves, in which order, and what is left alone, as the preview showed.
    if Plan::make(&paths, &repo, &manifest, &content)? != plan {
        return Err(Failure::refused(
            "the tool directories changed after the preview; nothing was changed, run the import again",
        ));
    }
    refuse_special_files(&plan)?;
    let staging = Staging::begin(&paths, &repo, &lock)?;
    let backup = Backup::create(&paths, "nothing was changed")?;
    let done = match &writes {
        Some((candidate, todo)) => write_repo(&repo, candidate, todo, &staging, &backup),
        None => Ok(()),
    }
    .and_then(|()| take_over(&plan, &backup));
    drop(staging);
    // Only an empty backup folder goes: nothing this run moved is in it.
    let unused = std::fs::remove_dir(backup.dir()).is_ok();
    let originals = if unused {
        "this run moved no original".to_owned()
    } else {
        format!(
            "the originals this run moved are in {}",
            backup.dir().display()
        )
    };
    if let Err(failure) = done {
        return Err(Failure {
            exit: failure.exit,
            message: format!(
                "{}\nthe import stopped part-way: the lines above list what was done; {originals}; run the same command again to finish it (it keeps what is already in place)",
                failure.message,
            ),
        });
    }
    let exit = sync::run(&paths, &repo, &lock, Output::Normal);
    if writes.is_some() {
        out!("imported {}; {originals}", folder.display());
    } else {
        out!(
            "the skill repo {} already held this import; {originals}",
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
    skills: BTreeMap<String, Snapshot>,
    shared_body: Vec<u8>,
    manifest: String,
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
        let mut names = Vec::new();
        if tree.kind("skills").map_err(io)? == Some(Kind::Dir) {
            for entry in tree.children("skills").map_err(io)? {
                if tree::is_ignored(&entry.path) {
                    continue;
                }
                if entry.kind == Kind::Dir {
                    names.push(tree::basename(&entry.path).to_owned());
                } else {
                    problems.push(format!(
                        "{} is not a folder; every entry in skills/ is a skill folder",
                        entry.path
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
        let mut folded: BTreeMap<String, &str> = BTreeMap::new();
        for name in &names {
            let key = name.nfc().collect::<String>().to_lowercase();
            if let Some(other) = folded.insert(key, name) {
                refusals.push(format!(
                    "{} holds skills/{other} and skills/{name}, which are the same path on a case- or normalisation-insensitive file system",
                    root.display()
                ));
            }
        }
        let mut skills = BTreeMap::new();
        for name in names {
            let dir = format!("skills/{name}");
            match path_safety::check(
                &tree,
                &dir,
                &root.join(&dir).display().to_string(),
                Content::Upstream,
            ) {
                Ok(()) => {
                    skills.insert(name, Snapshot::from_folder(&root.join(&dir))?);
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

/// The folder's `skiman.toml` with the skill repo's configuration merged in
/// by key, since an import brings skills and instructions only: the repo's
/// `[tools]` when the folder has none, each repo `[projects.<id>]` whose id
/// the folder does not define, and each repo `[[secrets.allow]]` entry for a
/// path below `projects/` the folder does not hold already. Tables written
/// inline count on both sides. The folder's own text keeps its bytes, except
/// an inline `projects` or `secrets` table that takes repo entries, which
/// becomes a standard table.
fn merge_repo_config(folder: &str, repo: &str) -> Result<String, Failure> {
    let Ok(mut doc) = folder.parse::<DocumentMut>() else {
        // The rules report the syntax error with its line.
        return Ok(folder.to_owned());
    };
    let Ok(repo) = repo.parse::<DocumentMut>() else {
        return Ok(folder.to_owned());
    };
    let mut merged = false;
    if !doc.contains_key("tools")
        && let Some(mut tools) = repo.get("tools").cloned()
    {
        if let Some(table) = tools.as_table_mut() {
            table.set_position(Some(-1));
        }
        doc.insert("tools", tools);
        merged = true;
    }
    if let Some(projects) = repo.get("projects").and_then(Item::as_table_like) {
        for (id, project) in projects.iter() {
            if doc.get("projects").and_then(|kept| kept.get(id)).is_none()
                && let Ok(project) = project.clone().into_table()
                && let Some(into) = manifest::subtable(&mut doc, "projects")
            {
                into.insert(id, Item::Table(appended(project)));
                merged = true;
            }
        }
    }
    let mirror = format!("{MIRROR_DIR}/");
    let allows = repo
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

/// The folder as the skill repo would hold it, with `manifest` as its `skiman.toml`.
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

/// One third-party skill's upstream, fetched at its declared ref.
struct Third {
    name: String,
    source: SourceRecord,
    /// `None` for a local-folder source.
    commit: Option<String>,
    /// The derived kind of `source.ref`.
    ref_kind: Option<RefKind>,
    snapshot: Snapshot,
}

impl Third {
    fn lock_entry(&self) -> LockEntry {
        LockEntry {
            commit: self.commit.clone(),
            git_ref: self.source.git_ref.clone(),
            snapshot: self.snapshot.tree_hash(),
        }
    }

    /// `<url> (<path>) at <commit>` or `<url> (local folder)`.
    fn describe(&self) -> String {
        let path = if self.source.path.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.source.path)
        };
        let version = match (&self.commit, self.ref_kind, &self.source.git_ref) {
            (None, _, _) => "(local folder)".to_owned(),
            (Some(commit), Some(RefKind::Commit), _) => format!("at {commit} (commit)"),
            (Some(commit), Some(kind), Some(r)) => format!("at {commit} ({} {r})", kind.as_str()),
            (Some(commit), _, _) => format!("at {commit} (default branch)"),
        };
        format!("{}{path} {version}", self.source.url)
    }
}

/// Fetches every declared source at its declared `ref` (the default branch
/// when there is none) and reads its snapshot; path safety runs on each.
fn fetch_sources(
    paths: &Paths,
    content: &Folder,
    manifest_text: &str,
    prompts: Prompts,
) -> Result<Vec<Third>, Failure> {
    let manifest = Manifest::parse(manifest_text)
        .map_err(|e| Failure::refused(format!("skiman.toml: {e}")))?;
    let mut thirds = Vec::new();
    for (name, source) in Folder::sources(&manifest) {
        let refuse = |why: &str| {
            Failure::refused(format!(
                "refusing to import {}: skiman.toml: [skills.{name}.source]: {why}; nothing was changed",
                content.root.display()
            ))
        };
        let url = &source.url;
        let kind = check_url(url)
            .map_err(|why| refuse(&format!("unsupported `url` `{url}`: it {why}")))?;
        if kind == UrlKind::Local {
            if source.git_ref.is_some() {
                return Err(refuse("a local folder has no versions; drop `ref`"));
            }
            let dir = if url == "~" {
                paths.home().to_path_buf()
            } else if let Some(rest) = url.strip_prefix("~/") {
                paths.home().join(rest)
            } else if url.starts_with('~') {
                return Err(refuse(
                    "`url` uses `~user`; give an absolute path or one below `~`",
                ));
            } else {
                PathBuf::from(url)
            };
            let dir = if source.path.is_empty() {
                dir
            } else {
                dir.join(&source.path)
            };
            thirds.push(Third {
                snapshot: Snapshot::read_local_upstream(&dir)?,
                name,
                source,
                commit: None,
                ref_kind: None,
            });
            continue;
        }
        let cache = Cache::open(paths, url, prompts)?;
        let (commit, ref_kind) = match &source.git_ref {
            Some(r) => {
                let resolved = cache.resolve_ref(r)?;
                (resolved.commit, Some(resolved.kind))
            }
            None => (cache.default_branch()?.commit, None),
        };
        cache.fetch_commit(&commit)?;
        thirds.push(Third {
            snapshot: Snapshot::read(&cache, &commit, &source.path)?,
            name,
            source,
            commit: Some(commit),
            ref_kind,
        });
    }
    Ok(thirds)
}

/// `manifest` with each source's `ref_kind` set to the derived kind, or
/// removed when the source has no `ref` (a hand-written `ref_kind` is never trusted).
fn with_ref_kinds(manifest: &str, thirds: &[Third]) -> Result<String, Failure> {
    let mut doc = manifest
        .parse::<DocumentMut>()
        .map_err(|e| Failure::refused(format!("skiman.toml: {e}")))?;
    let mut changed = false;
    for third in thirds {
        let Some(source) = doc
            .get_mut("skills")
            .and_then(|s| s.get_mut(&third.name))
            .and_then(|s| s.get_mut("source"))
            .and_then(Item::as_table_like_mut)
        else {
            continue;
        };
        let derived = third.ref_kind.map(RefKind::as_str);
        let current = source.get("ref_kind").and_then(Item::as_str);
        if current == derived {
            continue;
        }
        changed = true;
        match derived {
            Some(kind) => {
                source.insert("ref_kind", value(kind));
            }
            None => {
                source.remove("ref_kind");
            }
        }
    }
    Ok(if changed {
        doc.to_string()
    } else {
        manifest.to_owned()
    })
}

/// The `skiman.lock` text with one entry per third-party skill.
fn lock_text(thirds: &[Third]) -> Result<String, Failure> {
    let mut text = format!("version = {}\n", 1);
    for third in thirds {
        text = lockfile::with_entry(&text, &third.name, &third.lock_entry())
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
    /// what a link resolves through is a real path.
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
/// out again under the repo lock: a plan that differs from the previewed
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
}

impl Plan {
    /// Classifies every entry of every installed tool's tool directory (tool
    /// directories resolving to one real directory once) and every
    /// instruction location, as sync would after the import.
    fn make(
        paths: &Paths,
        repo: &SkillRepo,
        manifest: &Manifest,
        content: &Folder,
    ) -> Result<Plan, Failure> {
        let home = paths.home();
        let entries = manifest.skills();
        let mut wanted: BTreeMap<String, PathBuf> = content
            .skills
            .keys()
            .filter(|name| entries.get(*name).is_none_or(|e| e.enabled))
            .map(|name| (name.clone(), repo.skill_dir(name)))
            .collect();
        wanted.insert(managing_skills::NAME.to_owned(), repo.managing_skills_dir());
        let ownership = Ownership::new(home, repo, manifest, None);
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
                    Entry::Foreign => {
                        let real = real.join(&name);
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
        let mut seen = BTreeSet::new();
        for tool in sync::installed_tools(home, manifest) {
            let path = home.join(tool.instruction_file);
            let Ok(location) = sync::instruction_location(&path) else {
                continue;
            };
            if !seen.insert(location.clone()) {
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
    for location in &plan.moves {
        let meta = std::fs::symlink_metadata(&location.path).map_err(|e| {
            Failure::operational(format!("cannot read {}: {e}", location.path.display()))
        })?;
        if meta.is_dir() {
            let tree = WorkingTree::new(&location.path);
            let entries = tree.walk("").map_err(|e| {
                Failure::operational(format!("cannot read {}: {e}", location.path.display()))
            })?;
            found.extend(
                entries
                    .into_iter()
                    .filter(|e| e.kind == Kind::Special)
                    .map(|e| location.path.join(e.path)),
            );
        } else if !meta.is_file() && !meta.is_symlink() {
            found.push(location.path.clone());
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = found.iter().map(|p| format!("  {}", p.display())).collect();
    Err(Failure::refused(format!(
        "refusing to import: these special files (FIFO, socket or device) are in entries the import would move to the backup; move them away by hand, nothing was changed:\n{}",
        lines.join("\n")
    )))
}

/// Where the import folder's `skiman.toml` (`folder`, merged with the skill
/// repo's as the import would write it) differs from the skill repo's
/// (`repo`) in what is distributed: `[tools] enabled` as a set, `enabled` of
/// each skill, and the `ref` of each source. Shown when the skill repo
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

/// The preview lines for what the import writes to the skill repo.
fn print_writes(repo: &SkillRepo, candidate: &Candidate, todo: &[(Piece, Step)]) {
    let step = |piece: Piece| todo.iter().find(|(p, _)| *p == piece).map(|(_, step)| step);
    for (name, skill) in &candidate.content.skills {
        let verb = match step(Piece::Skill(name.clone())) {
            Some(Step::Keep) => "keep",
            _ => "add",
        };
        match candidate.thirds.iter().find(|t| &t.name == name) {
            None => out!("  {verb} skill {name} (own)"),
            Some(third) => {
                let changes = local_modifications(skill, &third.snapshot);
                let summary = if changes.is_empty() {
                    "; no file differs from upstream"
                } else {
                    "; files that differ from upstream:"
                };
                out!(
                    "  {verb} skill {name} (third-party, from {}){summary}",
                    third.describe()
                );
                for change in changes {
                    out!("      {change}");
                }
                for submodule in &third.snapshot.submodules {
                    out!("      {submodule}  [submodule, not fetched]");
                }
            }
        }
    }
    for (piece, step) in todo {
        if matches!(step, Step::Replace(_)) {
            out!(
                "  move {} to the backup, then write the import's",
                repo.root().join(piece.rel()).display()
            );
        }
    }
}

/// The preview lines for the tool directories and instruction locations.
fn print_plan(repo: &SkillRepo, plan: &Plan, paths: &Paths) {
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
    out!(
        "originals go to a new folder in {}",
        paths.state_dir().join("backups").display()
    );
}

/// One piece of skill repo content the import writes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Upstream(String),
    Lock,
    SharedBody,
    Manifest,
    Skill(String),
}

impl Piece {
    /// The path relative to the skill repo.
    fn rel(&self) -> String {
        match self {
            Piece::Upstream(name) => format!("upstream/{name}"),
            Piece::Lock => rules::LOCKFILE.to_owned(),
            Piece::SharedBody => SHARED_BODY.to_owned(),
            Piece::Manifest => rules::MANIFEST.to_owned(),
            Piece::Skill(name) => format!("skills/{name}"),
        }
    }
}

/// What a piece holds as the import writes it.
enum PieceContent<'a> {
    Folder(&'a Snapshot),
    File(&'a [u8]),
}

impl Candidate<'_> {
    /// Every piece, in the order the import writes them. [`progress`]
    /// depends on that order:
    /// - `skiman.toml` comes before the shared body, so a skill repo whose
    ///   shared body is the import's holds the import's `skiman.toml` too.
    ///   That holds for a run that starts with the skills, the source tables
    ///   or the shared body to write (each comes with `skiman.toml` or after
    ///   it), not for one that starts with only Consistency broken (a
    ///   `skiman.lock` deleted after the import, say): stopped right before
    ///   `skiman.toml`, it is taken as done when run again.
    /// - The skills come last: written before a shared body that held
    ///   something else, a run stopped between the two would be refused when
    ///   run again.
    fn pieces(&self) -> Vec<(Piece, PieceContent<'_>)> {
        let mut pieces: Vec<(Piece, PieceContent)> = self
            .thirds
            .iter()
            .map(|t| {
                (
                    Piece::Upstream(t.name.clone()),
                    PieceContent::Folder(&t.snapshot),
                )
            })
            .collect();
        pieces.extend([
            (Piece::Lock, PieceContent::File(self.lock.as_bytes())),
            (
                Piece::Manifest,
                PieceContent::File(self.manifest.as_bytes()),
            ),
            (
                Piece::SharedBody,
                PieceContent::File(&self.content.shared_body),
            ),
        ]);
        pieces.extend(
            self.content
                .skills
                .iter()
                .map(|(name, skill)| (Piece::Skill(name.clone()), PieceContent::Folder(skill))),
        );
        pieces
    }
}

/// What the import does with one piece, decided by what the skill repo holds.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// Nothing is there: write it.
    Write,
    /// Something else is there, which goes to the backup first: a file (its
    /// content) or an upstream snapshot (its tree hash).
    Replace(Vec<u8>),
    /// It is there as the import writes it.
    Keep,
}

/// What the skill repo holds where a piece goes.
#[derive(Debug, PartialEq, Eq)]
enum Held {
    Nothing,
    /// A folder where a file goes, or anything else that is not of the piece's kind.
    WrongType,
    /// The piece as the import writes it.
    Same,
    /// Something else: a file's content, or a folder's tree hash.
    Other(Vec<u8>),
}

/// What the skill repo holds at `path` against `want`: a folder compares
/// by tree hash, a file by its bytes.
fn held(path: &Path, want: &PieceContent) -> Result<Held, Failure> {
    let io =
        |e: std::io::Error| Failure::operational(format!("cannot read {}: {e}", path.display()));
    let meta = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Held::Nothing),
        Err(e) => return Err(io(e)),
        Ok(meta) => meta,
    };
    let now = match want {
        PieceContent::Folder(_) if !meta.is_dir() => return Ok(Held::WrongType),
        PieceContent::File(_) if !meta.is_file() => return Ok(Held::WrongType),
        PieceContent::Folder(snapshot) => {
            let now = tree::tree_hash_of_dir(path).map_err(io)?;
            if now == snapshot.tree_hash() {
                return Ok(Held::Same);
            }
            now.into_bytes()
        }
        PieceContent::File(bytes) => {
            let now = std::fs::read(path).map_err(io)?;
            if now == *bytes {
                return Ok(Held::Same);
            }
            now
        }
    };
    Ok(Held::Other(now))
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
/// the skill repo is there but is not a regular file.
fn refuse_irregular_files(repo: &SkillRepo) -> Result<(), Failure> {
    let mut lines = Vec::new();
    for rel in [rules::MANIFEST, rules::LOCKFILE, SHARED_BODY] {
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
        "the skill repo {} holds a link, folder or special file where import needs a regular file; replace each with a regular file or move it away, nothing was changed:\n{}",
        repo.root().display(),
        lines.join("\n")
    )))
}

/// How far the skill repo is with this import, decided before anything is fetched.
#[derive(Debug, PartialEq, Eq)]
enum Progress {
    /// The repo holds everything the import writes.
    Done,
    /// The repo holds everything but the import's shared body, and its own
    /// holds more than whitespace.
    OtherSharedBody,
    /// Anything else: the whole import is worked out again ([`repo_todo`]).
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
    repo: &SkillRepo,
    content: &Folder,
    folder_manifest: &Manifest,
    repo_manifest: &str,
) -> Result<Progress, Failure> {
    if !names_in(&repo.skills_dir())?
        .iter()
        .eq(content.skills.keys())
    {
        return Ok(Progress::NotDone);
    }
    for (name, skill) in &content.skills {
        if held(&repo.skill_dir(name), &PieceContent::Folder(skill))? != Held::Same {
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
        match held(
            &repo.shared_body(),
            &PieceContent::File(&content.shared_body),
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
fn other_shared_body(repo: &SkillRepo, folder: &Path) -> Failure {
    let theirs = folder.join(SHARED_BODY).display().to_string();
    let ours = repo.shared_body().display().to_string();
    Failure::refused(format!(
        "refusing to import {}: the skill repo {} already holds everything this import writes except {SHARED_BODY}, which differs from the import folder's; nothing was changed.\n  \
         If this import has run on this skill repo before (it finished or stopped part-way), make the two files the same and run it again: it finishes what is left and writes nothing else to the skill repo, whose skiman.toml stays as it is.\n    \
         to keep the skill repo's shared body, copy {ours} over {theirs};\n    \
         to use the import folder's, copy {theirs} over {ours} (skiman keeps no copy of the file you overwrite).\n  \
         If it has never run on this skill repo, empty {ours} (put what it holds into the import folder's shared body first if you want to keep it) and run it again: it imports everything, the import folder's skiman.toml included.\n  \
         If you cannot tell which, do the first.",
        folder.display(),
        repo.root().display(),
    ))
}

/// What to do with each piece. Refuses (exit 1, nothing changed), naming
/// each, a `skills/<name>` that differs from the folder's, anything in
/// `skills/`, `upstream/` or `skiman.lock` that the import does not write,
/// and a piece that is not a regular file or folder.
fn repo_todo(repo: &SkillRepo, candidate: &Candidate) -> Result<Vec<(Piece, Step)>, Failure> {
    let mut todo = Vec::new();
    let mut lines = Vec::new();
    for (piece, content) in candidate.pieces() {
        let rel = piece.rel();
        let step = match (held(&repo.root().join(&rel), &content)?, content) {
            (Held::Nothing, _) => Step::Write,
            (Held::Same, _) => Step::Keep,
            (Held::WrongType, PieceContent::Folder(_)) => {
                lines.push(format!("{rel} is not a folder"));
                continue;
            }
            (Held::WrongType, PieceContent::File(_)) => {
                lines.push(format!("{rel} is not a regular file"));
                continue;
            }
            (Held::Other(_), _) if matches!(piece, Piece::Skill(_)) => {
                lines.push(format!("{rel} differs from the import folder's {rel}"));
                continue;
            }
            (Held::Other(old), _) => Step::Replace(old),
        };
        todo.push((piece, step));
    }
    let third = |name: &str| candidate.thirds.iter().any(|t| t.name == name);
    for root in ["skills", "upstream"] {
        let names = names_in(&repo.root().join(root))?
            .into_iter()
            .filter(|name| match root {
                "skills" => !candidate.content.skills.contains_key(name),
                _ => !third(name),
            });
        lines.extend(names.map(|name| match root {
            "skills" => format!("{root}/{name} is not in the import folder"),
            _ => format!(
                "{root}/{name} is the snapshot of no third-party skill in the import folder"
            ),
        }));
    }
    for name in Lockfile::load(repo)?.skill_names() {
        if !third(&name) {
            lines.push(format!(
                "skiman.lock has an entry for {name}, which is no third-party skill in the import folder"
            ));
        }
    }
    if lines.is_empty() {
        return Ok(todo);
    }
    let lines: Vec<String> = lines.iter().map(|l| format!("  {l}")).collect();
    Err(Failure::refused(format!(
        "the skill repo {} holds content that is not this import's; import works only on a skill repo that holds nothing else: remove each of these from the skill repo, or make the import folder hold the same; nothing was changed:\n{}",
        repo.root().display(),
        lines.join("\n")
    )))
}

/// Writes the pieces that are not in place yet, in order, each staged under
/// `.skiman/tmp/` and renamed into place. What a piece replaces goes to
/// the backup first.
fn write_repo(
    repo: &SkillRepo,
    candidate: &Candidate,
    todo: &[(Piece, Step)],
    staging: &Staging,
    backup: &Backup,
) -> Result<(), Failure> {
    let stage = staging.dir();
    let pieces = candidate.pieces();
    let writes = || {
        pieces
            .iter()
            .zip(todo)
            .filter(|(_, (_, step))| *step != Step::Keep)
    };
    for ((piece, content), _) in writes() {
        let path = stage.join(piece.rel());
        match content {
            PieceContent::Folder(snapshot) => fsutil::write_files(&path, &snapshot.files)?,
            PieceContent::File(bytes) => fsutil::write_file(&path, false, bytes)?,
        }
    }
    for ((piece, content), (_, step)) in writes() {
        let rel = piece.rel();
        let target = repo.root().join(&rel);
        let staged = stage.join(&rel);
        let saved = Path::new("repo").join(&rel);
        let cannot_save = |e: std::io::Error| {
            Failure::operational(format!(
                "cannot move {} to the backup: {e}",
                target.display()
            ))
        };
        match (step, content) {
            // A file is replaced in one step, so the skill repo is never
            // without its `skiman.toml`: the old content is written to the
            // backup, then the new file is renamed over it.
            (Step::Replace(old), PieceContent::File(_)) => {
                let dest = backup.save(&saved, old).map_err(cannot_save)?;
                out!("saved the old {} as {}", target.display(), dest.display());
                test_hook::at("import", &format!("rename:{rel}"));
                std::fs::rename(&staged, &target).map_err(|e| {
                    Failure::operational(format!(
                        "cannot rename {} to {}: {e}",
                        staged.display(),
                        target.display()
                    ))
                })?;
            }
            (step, _) => {
                if *step != Step::Write {
                    let dest = backup.move_in(&target, &saved).map_err(cannot_save)?;
                    out!("moved {} to {}", target.display(), dest.display());
                }
                test_hook::at("import", &format!("rename:{rel}"));
                staging::rename_into_place(&staged, &target)?;
            }
        }
        out!("created {}", target.display());
    }
    Ok(())
}

/// Moves each original of `plan` to the backup. Sync creates the owned link
/// in its place. An original that is gone needs nothing.
fn take_over(plan: &Plan, backup: &Backup) -> Result<(), Failure> {
    for entry in &plan.moves {
        let point = entry.backup.display();
        test_hook::at("import", &format!("back-up:{point}"));
        if std::fs::symlink_metadata(&entry.path).is_ok() {
            let dest = backup.move_in(&entry.path, &entry.backup).map_err(|e| {
                Failure::operational(format!(
                    "cannot move {} to the backup: {e}",
                    entry.path.display()
                ))
            })?;
            out!("moved {} to {}", entry.path.display(), dest.display());
        }
        test_hook::at("import", &format!("moved:{point}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Move, merge_repo_config, move_order, resolution};

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
    fn a_link_to_a_link_to_a_folder_moves_before_both_and_an_entry_nothing_passes_through_keeps_its_place()
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
    fn merging_the_skill_repos_configuration_into_its_own_result_changes_no_byte() {
        let tools = "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n";
        let allow = "[[secrets.allow]]\npath = \"projects/proj/AGENTS.md\"\nrule = \"aws-access-key\"\nfingerprint = \"sha256:00\"\n";
        let proj = "[projects.proj]\npath = \"~/proj\"\n";
        // (the skill repo's skiman.toml, the import folder's)
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
        ];
        for (repo, folder) in cases {
            let merged = merge_repo_config(&folder, &repo).unwrap();
            assert_eq!(
                merge_repo_config(&folder, &merged).unwrap(),
                merged,
                "repo:\n{repo}\nfolder:\n{folder}"
            );
        }
    }
}
