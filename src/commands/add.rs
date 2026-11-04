//! `skiman add <source>`: install a third-party skill. Fetches the upstream,
//! shows the file list, then writes `skills/<name>/`, `upstream/<name>/`,
//! the `[skills.<name>.source]` table and the lock entry, and syncs. Never
//! commits.

use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::Exit;
use crate::commands::has_terminal;
use crate::failure::{Failure, finish};
use crate::fetch::{BranchHead, Cache};
use crate::fsutil::write_atomic;
use crate::git::Prompts;
use crate::lockfile::{self, LockEntry};
use crate::manifest::{self, SourceRecord};
use crate::names;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, Scope};
use crate::skill_md::SkillMd;
use crate::snapshot::Snapshot;
use crate::source::Source;
use crate::staging::{self, Staging};
use crate::sync::{self, Output};
use crate::tree::{self, Overlay, WorkingTree};

#[derive(Debug, Clone)]
pub struct Args {
    pub source: String,
    pub name: Option<String>,
    pub dry_run: bool,
    pub yes: bool,
}

pub fn run(args: Args) -> Exit {
    finish(add(args))
}

/// What `add` is about to install.
struct Install {
    source: Source,
    head: BranchHead,
    snapshot: Snapshot,
    /// The install name.
    name: String,
    upstream_name: String,
    /// `SKILL.md` for `skills/<name>` when its `name` line had to be set.
    renamed_skill_md: Option<String>,
}

fn add(args: Args) -> Result<Exit, Failure> {
    let terminal = has_terminal();
    if !terminal && !args.yes && !args.dry_run {
        return Err(Failure::usage(
            "missing flag without a terminal: --yes (install) or --dry-run (only show the file list)",
        ));
    }
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    let source = Source::parse(&args.source)?;
    if let Some(name) = &args.name {
        names::check_new_name(name)?;
    }
    let prompts = if terminal {
        Prompts::Allowed
    } else {
        Prompts::Never
    };
    let cache = Cache::open(&paths, &source.url, prompts)?;
    let head = cache.default_branch()?;
    cache.fetch_commit(&head.commit)?;
    let snapshot = Snapshot::read(&cache, &head.commit, &source.path)?;
    let install = inspect(source, head, snapshot, args.name)?;
    check_install_name(&repo, &install)?;

    print_file_list(&install);
    if args.dry_run {
        println!("dry run: nothing was written");
        return Ok(Exit::Success);
    }
    if !args.yes && !confirm(&format!("Install skill {}? [y/N] ", install.name))? {
        return Err(Failure::refused("not installed; nothing was written"));
    }

    let lock = RepoLock::wait(&repo)?;
    check_install_name(&repo, &install)?;
    let files = candidate_files(&install);
    let records = Records::build(&repo, &install, &files)?;
    validate(&repo, &install, &files, &records)?;
    let staging = Staging::begin(&paths, &repo, &lock)?;
    write(&repo, &install, &files, &records, &staging)?;
    drop(staging);
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}

/// Reads the upstream `SKILL.md` and decides the install name.
fn inspect(
    source: Source,
    head: BranchHead,
    snapshot: Snapshot,
    name: Option<String>,
) -> Result<Install, Failure> {
    let Some(file) = snapshot.file("SKILL.md") else {
        return Err(Failure::refused(format!(
            "{} has no SKILL.md at its root, so it is not a single skill",
            source.url
        )));
    };
    let skill_md = SkillMd::parse(&format!("SKILL.md in {}", source.url), &file.content)?;
    let upstream_name = skill_md.name().unwrap_or(&source.folder_name).to_owned();
    let name = name.unwrap_or_else(|| upstream_name.clone());
    let renamed_skill_md = if skill_md.name() == Some(name.as_str()) {
        None
    } else {
        Some(skill_md.with_name(&name)?)
    };
    Ok(Install {
        source,
        head,
        snapshot,
        name,
        upstream_name,
        renamed_skill_md,
    })
}

/// Refuses (exit 1) an install name that is invalid, reserved or taken,
/// pointing to `--name`.
fn check_install_name(repo: &SkillRepo, install: &Install) -> Result<(), Failure> {
    names::check_new_name(&install.name)
        .and_then(|()| names::check_not_taken(repo, &install.name))
        .map_err(|failure| {
            Failure::refused(format!(
                "{}; install it under another name with `--name <name>`",
                failure.message
            ))
        })
}

/// The full file list, with scripts and executables flagged.
fn print_file_list(install: &Install) {
    let upstream_name = if install.upstream_name == install.name {
        String::new()
    } else {
        format!(" (upstream name {})", install.upstream_name)
    };
    println!(
        "skill {}{upstream_name} from {} at {} (default branch {}):",
        install.name, install.source.url, install.head.commit, install.head.branch
    );
    for file in &install.snapshot.files {
        let mut flags = Vec::new();
        if is_script(&file.path, &file.content) {
            flags.push("script");
        }
        if file.executable {
            flags.push("executable");
        }
        if flags.is_empty() {
            println!("  {}", file.path);
        } else {
            println!("  {}  [{}]", file.path, flags.join(", "));
        }
    }
    for submodule in &install.snapshot.submodules {
        println!("  {submodule}  [submodule, not fetched]");
    }
    if install.renamed_skill_md.is_some() {
        println!(
            "the `name` line of skills/{0}/SKILL.md is set to `{0}` (a local modification; upstream/{0} keeps the upstream text)",
            install.name
        );
    }
}

/// A file agents may run: it starts with `#!` or has a script extension.
fn is_script(path: &str, content: &[u8]) -> bool {
    const EXTENSIONS: &[&str] = &[
        "sh",
        "bash",
        "zsh",
        "fish",
        "py",
        "rb",
        "pl",
        "php",
        "js",
        "mjs",
        "cjs",
        "ts",
        "ps1",
        "bat",
        "cmd",
        "applescript",
        "scpt",
    ];
    content.starts_with(b"#!")
        || tree::basename(path)
            .rsplit_once('.')
            .is_some_and(|(_, ext)| EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn confirm(question: &str) -> Result<bool, Failure> {
    let io = |e: std::io::Error| Failure::operational(format!("cannot read the answer: {e}"));
    eprint!("{question}");
    std::io::stderr().flush().map_err(io)?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(io)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// The new `skiman.toml` and `skiman.lock` texts.
struct Records {
    manifest: String,
    lock: String,
    lock_existed: bool,
}

impl Records {
    fn build(
        repo: &SkillRepo,
        install: &Install,
        files: &[(String, bool, Vec<u8>)],
    ) -> Result<Records, Failure> {
        let read = |path: &Path| match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Failure::operational(format!(
                "cannot read {}: {e}",
                path.display()
            ))),
        };
        let manifest_file = repo.manifest_file();
        let refuse = |path: &Path, e: String| Failure::refused(format!("{}: {e}", path.display()));
        let source = SourceRecord {
            url: install.source.url.clone(),
            path: install.source.path.clone(),
            git_ref: None,
            ref_kind: None,
            upstream_name: (install.upstream_name != install.name)
                .then(|| install.upstream_name.clone()),
        };
        let manifest = manifest::with_source(
            &read(&manifest_file)?.unwrap_or_default(),
            &install.name,
            &source,
        )
        .map_err(|e| refuse(&manifest_file, e))?;
        let lock_file = repo.lockfile();
        let old_lock = read(&lock_file)?;
        let entry = LockEntry {
            commit: Some(install.head.commit.clone()),
            git_ref: None,
            snapshot: tree::tree_hash(
                &overlay(&WorkingTree::new(repo.root()), files, Vec::new()),
                &format!("upstream/{}", install.name),
            )
            .map_err(|e| Failure::operational(format!("cannot hash the snapshot: {e}")))?,
        };
        let lock = lockfile::with_entry(
            old_lock.as_deref().unwrap_or_default(),
            &install.name,
            &entry,
        )
        .map_err(|e| refuse(&lock_file, e))?;
        Ok(Records {
            manifest,
            lock,
            lock_existed: old_lock.is_some(),
        })
    }
}

/// Every file `add` writes laid over the skill repo's working tree, plus `extra`.
fn overlay<'a>(
    working_tree: &'a WorkingTree,
    files: &[(String, bool, Vec<u8>)],
    extra: Vec<(String, Vec<u8>)>,
) -> Overlay<'a> {
    let executable = files
        .iter()
        .filter(|(_, executable, _)| *executable)
        .map(|(path, _, _)| path.clone())
        .collect();
    let mut contents: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|(path, _, content)| (path.clone(), content.clone()))
        .collect();
    contents.extend(extra);
    Overlay::new(working_tree, contents).with_executable(executable)
}

/// Every file `add` writes, repo-relative: `(path, executable, content)`.
fn candidate_files(install: &Install) -> Vec<(String, bool, Vec<u8>)> {
    let mut out = Vec::new();
    for file in &install.snapshot.files {
        let skill_content = match (&install.renamed_skill_md, file.path.as_str()) {
            (Some(renamed), "SKILL.md") => renamed.clone().into_bytes(),
            _ => file.content.clone(),
        };
        out.push((
            format!("skills/{}/{}", install.name, file.path),
            file.executable,
            skill_content,
        ));
        out.push((
            format!("upstream/{}/{}", install.name, file.path),
            file.executable,
            file.content.clone(),
        ));
    }
    out
}

/// The rules on everything `add` writes, before anything is written. The
/// candidate output is complete here: every file of both folders is in
/// `scope.files` (the skill also in `scope.skills`), plus the new source
/// table and lock entry. The secret scan of the candidate output (#7) runs
/// on `scope.files` inside `rules::check`; a separate scan entry point
/// belongs here, on `candidate` and `scope.files`.
fn validate(
    repo: &SkillRepo,
    install: &Install,
    files: &[(String, bool, Vec<u8>)],
    records: &Records,
) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(repo.root());
    let texts = vec![
        (
            rules::MANIFEST.to_owned(),
            records.manifest.clone().into_bytes(),
        ),
        (
            rules::LOCKFILE.to_owned(),
            records.lock.clone().into_bytes(),
        ),
    ];
    let candidate = overlay(&working_tree, files, texts);
    let name = BTreeSet::from([install.name.clone()]);
    let scope = Scope {
        files: files.iter().map(|(path, _, _)| path.clone()).collect(),
        skills: name.clone(),
        manifest_tables: name.clone(),
        lock_entries: name.clone(),
        lock: !records.lock_existed,
        consistency: name,
        ..Scope::default()
    };
    rules::validate_write(&candidate, &scope)
}

/// Stages both folders under `tmp/<timestamp>/`, renames them into place,
/// then writes `skiman.toml` and, last, `skiman.lock`.
fn write(
    repo: &SkillRepo,
    install: &Install,
    files: &[(String, bool, Vec<u8>)],
    records: &Records,
    staging: &Staging,
) -> Result<(), Failure> {
    for (path, executable, content) in files {
        let file = staging.dir().join(path);
        let io = |e: std::io::Error| {
            Failure::operational(format!("cannot write {}: {e}", file.display()))
        };
        std::fs::create_dir_all(file.parent().expect("staged file has a parent")).map_err(io)?;
        std::fs::write(&file, content).map_err(io)?;
        let mode = if *executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).map_err(io)?;
    }
    for root in ["upstream", "skills"] {
        let rel = format!("{root}/{}", install.name);
        let target = repo.root().join(&rel);
        staging::rename_into_place(&staging.dir().join(&rel), &target)?;
        println!("created {}", target.display());
    }
    let write_file = |path: &Path, text: &str| {
        write_atomic(path, text.as_bytes())
            .map_err(|e| Failure::operational(format!("cannot write {}: {e}", path.display())))
    };
    write_file(&repo.manifest_file(), &records.manifest)?;
    println!("changed {}", repo.manifest_file().display());
    write_file(&repo.lockfile(), &records.lock)?;
    let verb = if records.lock_existed {
        "changed"
    } else {
        "created"
    };
    println!("{verb} {}", repo.lockfile().display());
    Ok(())
}
