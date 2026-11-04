//! `skiman import <folder>`: bring an existing setup in from a prepared
//! import folder (spec Import, ADR 0001), and `skiman import --guide`, the
//! guide an agent follows to prepare one (read-only).
//!
//! Order: no TTY and no `--yes` -> exit 2 -> config -> unfinished record ->
//! the skill repo holds no skills, snapshots or lock entries -> phase 1 on
//! the folder (layout, path safety, every rule except Consistency) -> fetch
//! every declared source -> phase 2 on the complete result (Consistency,
//! secrets, everything) -> takeover plan -> preview -> confirm -> repo lock,
//! checks again -> tmp leftovers -> import record and temporary area ->
//! `upstream/`, `skiman.lock`, `instructions/shared.md`, `skiman.toml`,
//! `skills/` (each staged under `.skiman/tmp/` and renamed into place) ->
//! takeover of locations -> record finished -> sync. Never commits.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, value};
use unicode_normalization::UnicodeNormalization;

use crate::Exit;
use crate::backup::Backup;
use crate::commands::has_terminal;
use crate::failure::{Failure, finish};
use crate::fetch::{Cache, RefKind};
use crate::fsutil;
use crate::git::Prompts;
use crate::import_guide;
use crate::import_record::{
    self, ImportRecord, Location, LocationState, RepoFiles, SkillHashes, Status,
};
use crate::lockfile::{self, LockEntry, Lockfile};
use crate::managing_skills;
use crate::manifest::{Manifest, SourceRecord};
use crate::ownership::{Entry, Ownership};
use crate::path_safety::{self, Content};
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::snapshot::{Snapshot, SnapshotFile};
use crate::staging::{self, Staging};
use crate::sync::{self, Output};
use crate::tree::{self, Kind, Overlay, Tree, WorkingTree};

pub fn guide() -> Exit {
    finish(
        std::io::stdout()
            .write_all(import_guide::text().as_bytes())
            .map(|()| Exit::Success)
            .map_err(|e| Failure::operational(format!("cannot print the guide: {e}"))),
    )
}

pub fn run(folder: &Path, yes: bool) -> Exit {
    finish(import(folder, yes))
}

const SHARED_BODY: &str = "instructions/shared.md";

fn import(folder: &Path, yes: bool) -> Result<Exit, Failure> {
    let terminal = has_terminal();
    if !terminal && !yes {
        return Err(Failure::usage(
            "missing flag without a terminal: --yes (import without asking for confirmation)",
        ));
    }
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    refuse_unfinished(&paths)?;
    let folder = folder.canonicalize().map_err(|e| {
        Failure::refused(format!(
            "the import folder {} is not reachable: {e}",
            folder.display()
        ))
    })?;
    let repo_files = check_repo_is_empty(&repo)?;
    let prompts = if terminal {
        Prompts::Allowed
    } else {
        Prompts::Never
    };

    let content = Folder::read(&folder)?;
    let manifest_text = merge_tools(&content.manifest, repo_files.manifest_text())?;
    check_folder(&content, &manifest_text)?;
    let thirds = fetch_sources(&paths, &content, &manifest_text, prompts)?;
    let manifest_text = with_ref_kinds(&manifest_text, &thirds)?;
    let lock_text = lock_text(&thirds)?;
    check_result(&content, &manifest_text, &lock_text, &thirds)?;
    let manifest = Manifest::parse(&manifest_text)
        .map_err(|e| Failure::refused(format!("skiman.toml: {e}")))?;
    let plan = Plan::make(&paths, &repo, &manifest, &content)?;
    refuse_special_files(&plan)?;

    let candidate = Candidate {
        content,
        manifest: manifest_text,
        lock: lock_text,
        thirds,
    };
    print_preview(&repo, &candidate, &plan, &repo_files, &paths);
    if !yes && !confirm(&format!("Import {}? [y/N] ", folder.display()))? {
        return Err(Failure::refused("not imported; nothing was changed"));
    }

    let lock = RepoLock::wait(&repo)?;
    refuse_unfinished(&paths)?;
    let repo_files_now = check_repo_is_empty(&repo)?;
    if repo_files_now != repo_files {
        return Err(Failure::refused(
            "the skill repo's instructions/shared.md, skiman.toml or skiman.lock changed after the preview; nothing was changed, run the import again",
        ));
    }
    if Folder::hash(&folder)? != candidate.content.hash {
        return Err(Failure::refused(format!(
            "the import folder {} changed after the preview; nothing was changed, run the import again",
            folder.display()
        )));
    }
    let staging = Staging::begin(&paths, &repo, &lock)?;
    let backup = Backup::create(&paths).map_err(|e| {
        Failure::operational(format!(
            "cannot create a backup folder in {}: {e}; nothing was changed",
            paths.state_dir().join("backups").display()
        ))
    })?;
    let mut record = record(&repo, &candidate, &plan, &repo_files, &backup);
    write_area(&paths, &candidate)?;
    record.save(&paths)?;
    println!(
        "saved the import record to {}",
        import_record::file(&paths).display()
    );
    let written = write_repo(&repo, &candidate, &repo_files, &staging, &backup, &paths)
        .and_then(|()| take_over(&paths, &mut record, &backup));
    drop(staging);
    if let Err(failure) = written {
        return Err(Failure {
            exit: failure.exit,
            message: format!(
                "{}\nthe import stopped part-way: the lines above list what was done; the originals moved so far are in {}; the import record is {}",
                failure.message,
                backup.dir().display(),
                import_record::file(&paths).display()
            ),
        });
    }
    record.status = Status::Finished;
    record.save(&paths)?;
    let _ = std::fs::remove_dir_all(import_record::area(&paths));
    let exit = sync::run(&paths, &repo, &lock, Output::Normal);
    println!(
        "imported {}; the originals are in {}",
        folder.display(),
        backup.dir().display()
    );
    println!("the import folder {} can be deleted", folder.display());
    Ok(exit)
}

/// Refuses (exit 1) while an import that stopped part-way is recorded.
fn refuse_unfinished(paths: &Paths) -> Result<(), Failure> {
    match ImportRecord::load(paths)? {
        Some(record) if record.status == Status::InProgress => Err(Failure::refused(format!(
            "an import of {} into {} stopped part-way (record {}); this version of skiman cannot resume it",
            record.folder.display(),
            record.repo.display(),
            import_record::file(paths).display()
        ))),
        _ => Ok(()),
    }
}

/// The skill repo's files an import replaces, as they are before it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoFilesNow {
    shared_body: Option<Vec<u8>>,
    manifest: Option<Vec<u8>>,
    lock: Option<Vec<u8>>,
}

impl RepoFilesNow {
    fn manifest_text(&self) -> Option<String> {
        self.manifest
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }

    fn hashes(&self) -> RepoFiles {
        RepoFiles {
            shared_body: self.shared_body.as_deref().map(file_hash),
            manifest: self.manifest.as_deref().map(file_hash),
            lock: self.lock.as_deref().map(file_hash),
        }
    }
}

fn file_hash(content: &[u8]) -> String {
    format!("sha256:{}", tree::hex(&Sha256::digest(content)))
}

/// Refuses (exit 1) a skill repo that is not initialised, or that holds a
/// skill, an upstream snapshot or a lock entry. Returns its replaced files.
fn check_repo_is_empty(repo: &SkillRepo) -> Result<RepoFilesNow, Failure> {
    let read = |path: &Path| {
        let io = |e: std::io::Error| {
            Failure::operational(format!("cannot read {}: {e}", path.display()))
        };
        match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
            // A special file is never opened (a FIFO would block) nor moved.
            Ok(meta) if !meta.is_file() => {
                return Err(Failure::refused(format!(
                    "{} is not a regular file; move it away by hand, nothing was changed",
                    path.display()
                )));
            }
            Ok(_) => {}
        }
        std::fs::read(path).map(Some).map_err(io)
    };
    let files = RepoFilesNow {
        shared_body: read(&repo.shared_body())?,
        manifest: read(&repo.manifest_file())?,
        lock: read(&repo.lockfile())?,
    };
    if files.manifest.is_none() {
        return Err(Failure::refused(format!(
            "{} is not an initialised skill repo (it has no skiman.toml); run `skiman init {}`",
            repo.root().display(),
            repo.root().display()
        )));
    }
    let refuse = |what: String| {
        Failure::refused(format!(
            "the skill repo {} already holds {what}; import works only on a skill repo without skills, upstream snapshots and lock entries; nothing was changed",
            repo.root().display()
        ))
    };
    for root in ["skills", "upstream"] {
        let dir = repo.root().join(root);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
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
        if let Some(name) = names.first() {
            return Err(refuse(format!("{root}/{name}")));
        }
    }
    if let Some(name) = Lockfile::load(repo)?.skill_names().first() {
        return Err(refuse(format!("a skiman.lock entry for {name}")));
    }
    Ok(files)
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

/// The folder's `skiman.toml`, with the repo's `[tools]` table added when
/// the folder has none. Everything else keeps its bytes.
fn merge_tools(folder: &str, repo: Option<String>) -> Result<String, Failure> {
    let Ok(mut doc) = folder.parse::<DocumentMut>() else {
        // The rules report the syntax error with its line.
        return Ok(folder.to_owned());
    };
    if doc.contains_key("tools") {
        return Ok(folder.to_owned());
    }
    let tools = repo
        .and_then(|text| text.parse::<DocumentMut>().ok())
        .and_then(|repo| repo.get("tools").cloned());
    let Some(mut tools) = tools else {
        return Ok(folder.to_owned());
    };
    if let Some(table) = tools.as_table_mut() {
        table.set_position(Some(-1));
    }
    doc.insert("tools", tools);
    Ok(doc.to_string())
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
        if url.starts_with('/') || url.starts_with('~') {
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
                snapshot: Snapshot::read_local(&dir)?,
                name,
                source,
                commit: None,
                ref_kind: None,
            });
            continue;
        }
        let is_git_url = (url.starts_with("https://") || url.starts_with("ssh://"))
            || url
                .split_once(':')
                .is_some_and(|(user_host, _)| user_host.contains('@') && !user_host.contains('/'));
        if !is_git_url || url.contains("::") || url.starts_with('-') {
            return Err(refuse(&format!(
                "unsupported `url` `{url}`: give an https:// or ssh:// git URL, user@host:path, or a local absolute or ~ path"
            )));
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
struct Candidate {
    content: Folder,
    /// The `skiman.toml` text the import writes.
    manifest: String,
    /// The `skiman.lock` text the import writes.
    lock: String,
    thirds: Vec<Third>,
}

/// What happens to the tool directories and instruction locations.
#[derive(Debug, Default)]
struct Plan {
    /// Entries and instruction files moved to the backup, in order.
    moves: Vec<Location>,
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
        let ownership = Ownership::new(home, repo, None);
        let io = |path: &Path, e: std::io::Error| {
            Failure::operational(format!("cannot read {}: {e}", path.display()))
        };
        let mut plan = Plan::default();
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
                    Entry::Foreign => plan.moves.push(Location {
                        tool: tool.id.to_owned(),
                        path,
                        backup: PathBuf::from(tool.id).join(&name),
                        link: wanted.get(&text).cloned(),
                        state: LocationState::Planned,
                    }),
                    Entry::NotASkill if wanted.contains_key(&text) => {
                        plan.staying.push((path, text));
                    }
                    Entry::NotASkill => {}
                }
            }
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
            let meta = match std::fs::symlink_metadata(&location) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io(&location, e)),
            };
            if ownership.owned_instruction(&location, &meta).is_none() {
                let file_name = location.file_name().expect("location has a file name");
                plan.moves.push(Location {
                    tool: tool.id.to_owned(),
                    backup: PathBuf::from(tool.id).join(file_name),
                    path: location,
                    link: Some(repo.shared_body()),
                    state: LocationState::Planned,
                });
            }
        }
        Ok(plan)
    }
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

fn print_preview(
    repo: &SkillRepo,
    candidate: &Candidate,
    plan: &Plan,
    repo_files: &RepoFilesNow,
    paths: &Paths,
) {
    println!(
        "import {} into the skill repo {}:",
        candidate.content.root.display(),
        repo.root().display()
    );
    for (name, skill) in &candidate.content.skills {
        match candidate.thirds.iter().find(|t| &t.name == name) {
            None => println!("  add skill {name} (own)"),
            Some(third) => {
                let changes = local_modifications(skill, &third.snapshot);
                let summary = if changes.is_empty() {
                    "; no file differs from upstream"
                } else {
                    "; files that differ from upstream:"
                };
                println!(
                    "  add skill {name} (third-party, from {}){summary}",
                    third.describe()
                );
                for change in changes {
                    println!("      {change}");
                }
                for submodule in &third.snapshot.submodules {
                    println!("      {submodule}  [submodule, not fetched]");
                }
            }
        }
    }
    for (file, now) in [
        (repo.shared_body(), &repo_files.shared_body),
        (repo.manifest_file(), &repo_files.manifest),
        (repo.lockfile(), &repo_files.lock),
    ] {
        if now.is_some() {
            println!(
                "  move {} to the backup, then write the import's",
                file.display()
            );
        }
    }
    for location in &plan.moves {
        match &location.link {
            Some(target) if target == &repo.shared_body() => println!(
                "  move {} to the backup, then link it to the shared body",
                location.path.display()
            ),
            Some(_) => println!(
                "  move {} to the backup, then link it to skill {}",
                location.path.display(),
                tree::basename(&location.backup.to_string_lossy())
            ),
            None => println!(
                "  move {} to the backup (no imported skill of that name is distributed)",
                location.path.display()
            ),
        }
    }
    for path in &plan.skipped {
        println!("  skip {}: managed elsewhere", path.display());
    }
    for (path, name) in &plan.staying {
        println!(
            "  keep {}: it is not a skill, so skill {name} is not linked there",
            path.display()
        );
    }
    println!(
        "originals go to a new folder in {}",
        paths.state_dir().join("backups").display()
    );
}

fn confirm(question: &str) -> Result<bool, Failure> {
    let io = |e: std::io::Error| Failure::operational(format!("cannot read the answer: {e}"));
    eprint!("{question}");
    std::io::stderr().flush().map_err(io)?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(io)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// The import record as of confirmation: nothing written yet.
fn record(
    repo: &SkillRepo,
    candidate: &Candidate,
    plan: &Plan,
    repo_files: &RepoFilesNow,
    backup: &Backup,
) -> ImportRecord {
    let skills = candidate
        .content
        .skills
        .iter()
        .map(|(name, skill)| {
            let upstream = candidate
                .thirds
                .iter()
                .find(|t| &t.name == name)
                .map(|t| t.snapshot.tree_hash());
            (
                name.clone(),
                SkillHashes {
                    skill: skill.tree_hash(),
                    upstream,
                },
            )
        })
        .collect();
    ImportRecord {
        status: Status::InProgress,
        repo: repo.root().to_path_buf(),
        folder: candidate.content.root.clone(),
        folder_hash: candidate.content.hash.clone(),
        backup: backup.dir().to_path_buf(),
        pre_import: repo_files.hashes(),
        candidate: RepoFiles {
            shared_body: Some(file_hash(&candidate.content.shared_body)),
            manifest: Some(file_hash(candidate.manifest.as_bytes())),
            lock: Some(file_hash(candidate.lock.as_bytes())),
        },
        lock_content: candidate.lock.clone(),
        skills,
        locations: plan.moves.clone(),
    }
}

fn write_files(dir: &Path, files: &[SnapshotFile]) -> Result<(), Failure> {
    for file in files {
        write_file(&dir.join(&file.path), file.executable, &file.content)?;
    }
    if files.is_empty() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Failure::operational(format!("cannot create {}: {e}", dir.display())))?;
    }
    Ok(())
}

fn write_file(path: &Path, executable: bool, content: &[u8]) -> Result<(), Failure> {
    let io =
        |e: std::io::Error| Failure::operational(format!("cannot write {}: {e}", path.display()));
    std::fs::create_dir_all(path.parent().expect("file has a parent")).map_err(io)?;
    std::fs::write(path, content).map_err(io)?;
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(io)
}

/// Writes the temporary area: `upstream/<name>/` and `skiman.lock` as built.
fn write_area(paths: &Paths, candidate: &Candidate) -> Result<(), Failure> {
    let area = import_record::area(paths);
    match std::fs::remove_dir_all(&area) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(Failure::operational(format!(
                "cannot remove {}: {e}",
                area.display()
            )));
        }
    }
    for third in &candidate.thirds {
        write_files(
            &area.join("upstream").join(&third.name),
            &third.snapshot.files,
        )?;
    }
    write_file(
        &area.join(rules::LOCKFILE),
        false,
        candidate.lock.as_bytes(),
    )
}

/// Writes the skill repo content in the spec's order, each piece staged
/// under `.skiman/tmp/` and renamed into place; an existing file is moved
/// to the backup first.
fn write_repo(
    repo: &SkillRepo,
    candidate: &Candidate,
    repo_files: &RepoFilesNow,
    staging: &Staging,
    backup: &Backup,
    paths: &Paths,
) -> Result<(), Failure> {
    let area = import_record::area(paths);
    let stage = staging.dir();
    for third in &candidate.thirds {
        let snapshot = Snapshot::from_folder(&area.join("upstream").join(&third.name))?;
        write_files(&stage.join("upstream").join(&third.name), &snapshot.files)?;
    }
    let lock = std::fs::read(area.join(rules::LOCKFILE))
        .map_err(|e| Failure::operational(format!("cannot read {}: {e}", area.display())))?;
    write_file(&stage.join(rules::LOCKFILE), false, &lock)?;
    write_file(
        &stage.join(SHARED_BODY),
        false,
        &candidate.content.shared_body,
    )?;
    write_file(
        &stage.join(rules::MANIFEST),
        false,
        candidate.manifest.as_bytes(),
    )?;
    for (name, skill) in &candidate.content.skills {
        write_files(&stage.join("skills").join(name), &skill.files)?;
    }

    for third in &candidate.thirds {
        let rel = format!("upstream/{}", third.name);
        put_in_place(repo, stage, &rel, false, backup)?;
    }
    put_in_place(
        repo,
        stage,
        rules::LOCKFILE,
        repo_files.lock.is_some(),
        backup,
    )?;
    put_in_place(
        repo,
        stage,
        SHARED_BODY,
        repo_files.shared_body.is_some(),
        backup,
    )?;
    put_in_place(
        repo,
        stage,
        rules::MANIFEST,
        repo_files.manifest.is_some(),
        backup,
    )?;
    for name in candidate.content.skills.keys() {
        put_in_place(repo, stage, &format!("skills/{name}"), false, backup)?;
    }
    Ok(())
}

/// Renames `<stage>/<rel>` to `<repo>/<rel>`, first moving an existing
/// original to `<backup>/repo/<rel>` when `replaces`.
fn put_in_place(
    repo: &SkillRepo,
    stage: &Path,
    rel: &str,
    replaces: bool,
    backup: &Backup,
) -> Result<(), Failure> {
    let target = repo.root().join(rel);
    if replaces {
        let dest = backup
            .move_in(&target, &Path::new("repo").join(rel))
            .map_err(|e| {
                Failure::operational(format!(
                    "cannot move {} to the backup: {e}",
                    target.display()
                ))
            })?;
        println!("moved {} to {}", target.display(), dest.display());
    }
    staging::rename_into_place(&stage.join(rel), &target)?;
    println!("created {}", target.display());
    Ok(())
}

/// Moves each original to the backup, then creates the owned link in its
/// place, saving the record after each step.
fn take_over(paths: &Paths, record: &mut ImportRecord, backup: &Backup) -> Result<(), Failure> {
    for i in 0..record.locations.len() {
        let location = record.locations[i].clone();
        let dest = backup
            .move_in(&location.path, &location.backup)
            .map_err(|e| {
                Failure::operational(format!(
                    "cannot move {} to the backup: {e}",
                    location.path.display()
                ))
            })?;
        println!("moved {} to {}", location.path.display(), dest.display());
        record.locations[i].state = LocationState::BackedUp;
        record.save(paths)?;
        if let Some(target) = &location.link {
            fsutil::symlink_atomic(target, &location.path).map_err(|e| {
                Failure::operational(format!("cannot link {}: {e}", location.path.display()))
            })?;
            println!("linked {}", location.path.display());
        }
        record.locations[i].state = LocationState::TakenOver;
        record.save(paths)?;
    }
    Ok(())
}
