//! `skiman add <source> [--group <group>]`: install third-party skills.
//! Fetches the upstream (or reads a local folder), finds the skills in it,
//! lets the user choose, shows the file lists, then writes the skill folder
//! (`skills/[<group>/]<name>/`), `upstream/<name>/`, the
//! `[skills.<name>.source]` table and the lock entry of each, and syncs.
//! Never commits.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::Exit;
use crate::backup::Backups;
use crate::change::{self, Change, Held, Want};
use crate::cli::AddArgs as Args;
use crate::commands::{ask, confirm, has_terminal, prompts};
use crate::discovery;
use crate::failure::{Failure, finish};
use crate::fetch::Cache;
use crate::fsutil::read_text;
use crate::git::Prompts;
use crate::layout::skill_path;
use crate::lockfile::{self, LockEntry, Lockfile};
use crate::manifest::{self, Manifest, SourceRecord};
use crate::names::{self, Taken};
use crate::path_safety;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::repo_lock::RepoLock;
use crate::rules::{self, LIMITS, Scope};
use crate::skill_md::SkillMd;
use crate::snapshot::{Snapshot, SnapshotFile};
use crate::source::{Source, SourceKind};
use crate::sync::{self, Output};
use crate::tree::WorkingTree;
use crate::upstream::{Upstream, print_not_installed, run_flags};

pub fn run(args: Args) -> Exit {
    finish(add(args))
}

/// A folder of the upstream that holds a `SKILL.md`.
struct Candidate {
    /// Relative to the upstream's root: what `source.path` records.
    path: String,
    /// Its frontmatter `name`, else its folder name.
    upstream_name: String,
}

/// One skill `add` is about to install.
struct Install {
    /// `path` is the candidate folder.
    source: Source,
    snapshot: Snapshot,
    /// The install name.
    name: String,
    /// The skill folder it goes to, `skills/[<group>/]<name>`.
    skill: String,
    upstream_name: String,
    /// `SKILL.md` for the skill folder when its `name` line had to be set.
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
    let cwd = std::env::current_dir()
        .map_err(|e| Failure::operational(format!("cannot read the current folder: {e}")))?;
    let source = Source::parse(&args.source, paths.home(), &cwd)?;
    if let Some(name) = &args.name {
        names::check_new_name(name)?;
    }
    let prompts = prompts();
    let (source, upstream) = fetch(&paths, source, args.git_ref.as_deref(), prompts)?;
    let candidates = discover(&source, &upstream)?;

    let selected = if !args.skills.is_empty() {
        select_named(&source, &candidates, &args.skills)?
    } else if args.all || candidates.len() == 1 {
        candidates.iter().collect()
    } else if args.dry_run {
        print_candidates(&source, &upstream, &candidates, false);
        out!("dry run: nothing was written");
        return Ok(Exit::Success);
    } else if !terminal {
        return Err(Failure::usage(format!(
            "missing flag without a terminal: {} holds {} skills; choose with --skill <name> or --skill <folder>/ (repeatable), or --all",
            source.url,
            candidates.len()
        )));
    } else {
        print_candidates(&source, &upstream, &candidates, true);
        pick(&candidates)?
    };
    // Skills sharing one name are refused as such by `check_install_names`.
    let distinct: BTreeSet<_> = selected.iter().map(|c| &c.upstream_name).collect();
    if args.name.is_some() && selected.len() > 1 && distinct.len() == selected.len() {
        return Err(Failure::usage(
            "--name works with a single skill only; install the others under their upstream names first",
        ));
    }
    let name = args.name.filter(|_| selected.len() == 1);

    let mut installs = Vec::new();
    for candidate in selected {
        let snapshot = upstream.snapshot(&candidate.path)?;
        let source = source.clone().with_path(candidate.path.clone());
        let group = args.group.as_deref();
        installs.push(inspect(source, snapshot, candidate, name.clone(), group)?);
    }
    if let Some(group) = &args.group {
        let layout = repo.layout()?;
        for install in &installs {
            names::check_group(group, &install.name, &layout)?;
        }
    }
    check_install_names(paths.home(), &repo, &installs, &upstream)?;

    for install in &installs {
        print_file_list(install, &upstream);
    }
    if args.dry_run {
        out!("dry run: nothing was written");
        return Ok(Exit::Success);
    }
    let names: Vec<&str> = installs.iter().map(|i| i.name.as_str()).collect();
    let question = match names.as_slice() {
        [one] => format!("Install skill {one}? [y/N] "),
        many => format!("Install skills {}? [y/N] ", many.join(", ")),
    };
    if !args.yes && !confirm(&question)? {
        return Err(Failure::refused("not installed; nothing was written"));
    }

    let lock = RepoLock::wait(&paths, &repo)?;
    check_install_names(paths.home(), &repo, &installs, &upstream)?;
    let change = change(&repo, &installs, &upstream)?;
    validate(&repo, &installs, &change)?;
    change.apply(&repo, &lock, &mut Backups::new(&paths))?;
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}

/// Resolves the version to install and makes it readable: for a git source
/// the commit is fetched into the cache; a tree URL's path is split off here.
fn fetch(
    paths: &Paths,
    source: Source,
    git_ref: Option<&str>,
    prompts: Prompts,
) -> Result<(Source, Upstream), Failure> {
    match &source.kind {
        SourceKind::Local { dir } => {
            if git_ref.is_some() {
                return Err(Failure::refused(format!(
                    "{} is a local folder, which has no versions; drop --ref",
                    source.url
                )));
            }
            let dir = dir.clone();
            Ok((source, Upstream::Local { dir }))
        }
        SourceKind::Tree { tail } => {
            if git_ref.is_some() {
                return Err(Failure::refused(
                    "the URL already names a ref; drop --ref, or give the repository URL with --ref",
                ));
            }
            let cache = Cache::open(paths, &source.url, prompts)?;
            let (resolved, path) = cache.resolve_tree_tail(tail)?;
            let upstream = Upstream::at(cache, resolved.commit.clone(), Some(resolved), None)?;
            Ok((source.with_path(path), upstream))
        }
        SourceKind::Git => {
            let cache = Cache::open(paths, &source.url, prompts)?;
            Ok((source, Upstream::at_ref(cache, git_ref)?))
        }
    }
}

/// The candidate skills below `source.path`, with their upstream names. A
/// candidate whose `SKILL.md` cannot be read as frontmatter goes by its
/// folder name here; it is refused only if it is selected. A candidate whose
/// folder path has a name path safety refuses is skipped with a warning.
fn discover(source: &Source, upstream: &Upstream) -> Result<Vec<Candidate>, Failure> {
    let listing: BTreeMap<String, (String, u64)> = upstream
        .list(&source.path)?
        .into_iter()
        .map(|(path, key, size)| (path, (key, size)))
        .collect();
    let files: Vec<String> = listing.keys().cloned().collect();
    let mut folders = discovery::candidates(&files);
    folders.retain(|folder| {
        let path = discovery::join(&source.path, folder);
        let size = listing[&discovery::join(folder, "SKILL.md")].1;
        // `""` is the source folder itself.
        let skipped = if !folder.is_empty() && !path_safety::is_safe_path(folder) {
            path_safety::unsafe_path(&path)
        } else if size > LIMITS.file_bytes {
            let skill_md = discovery::join(&path, "SKILL.md");
            format!(
                "{skill_md} of {size} bytes, over the limit of {} bytes",
                LIMITS.file_bytes
            )
        } else {
            return true;
        };
        err!(
            "warning: {} holds {skipped}; the skill there is skipped",
            source.url
        );
        false
    });
    if folders.is_empty() {
        let at = if source.path.is_empty() {
            String::new()
        } else {
            format!(" below {}", source.path)
        };
        return Err(Failure::refused(format!(
            "no skill found in {}{at}: no folder holds a SKILL.md",
            source.url
        )));
    }
    let paths: Vec<String> = folders
        .iter()
        .map(|f| discovery::join(&source.path, f))
        .collect();
    // Every candidate was found by its listed `SKILL.md`.
    let skill_mds: Vec<&str> = folders
        .iter()
        .map(|f| listing[&discovery::join(f, "SKILL.md")].0.as_str())
        .collect();
    let contents = upstream.read_files(&skill_mds)?;
    Ok(paths
        .into_iter()
        .zip(contents)
        .map(|(path, content)| {
            let name = SkillMd::parse("SKILL.md", &content)
                .ok()
                .and_then(|md| md.name().map(str::to_owned));
            Candidate {
                upstream_name: name.unwrap_or_else(|| source.folder_name(&path)),
                path,
            }
        })
        .collect())
}

/// The candidates the `--skill` values name. A value without `/` is an
/// upstream name; one with `/` is a candidate's folder (`pdf/` for one at
/// the top), which tells apart two skills with the same name. An unknown
/// value is refused.
fn select_named<'a>(
    source: &Source,
    candidates: &'a [Candidate],
    values: &[String],
) -> Result<Vec<&'a Candidate>, Failure> {
    let names = |c: &Candidate, value: &str| match value.contains('/') {
        true => c.path == value.trim_end_matches('/'),
        false => c.upstream_name == value,
    };
    for value in values {
        if !candidates.iter().any(|c| names(c, value)) {
            let known: Vec<String> = candidates
                .iter()
                .map(|c| match c.path.as_str() {
                    "" => c.upstream_name.clone(),
                    path => format!("{} ({path}/)", c.upstream_name),
                })
                .collect();
            let what = if value.contains('/') {
                "in the folder"
            } else {
                "named"
            };
            return Err(Failure::refused(format!(
                "{} has no skill {what} `{value}`; it holds: {}",
                source.url,
                known.join(", ")
            )));
        }
    }
    Ok(candidates
        .iter()
        .filter(|c| values.iter().any(|value| names(c, value)))
        .collect())
}

/// The candidate list; `numbered` for the interactive pick.
fn print_candidates(
    source: &Source,
    upstream: &Upstream,
    candidates: &[Candidate],
    numbered: bool,
) {
    let choose = if numbered {
        ""
    } else {
        "; choose with --skill <name>, --skill <folder>/ or --all"
    };
    out!(
        "{} {} holds {} skills{choose}:",
        source.url,
        upstream.version(),
        candidates.len()
    );
    for (i, candidate) in candidates.iter().enumerate() {
        let number = if numbered {
            format!("{}. ", i + 1)
        } else {
            String::new()
        };
        out!(
            "  {number}{}  ({}/)",
            candidate.upstream_name,
            candidate.path
        );
    }
}

/// Asks which candidates to install: numbers separated by spaces, or `all`.
fn pick(candidates: &[Candidate]) -> Result<Vec<&Candidate>, Failure> {
    let answer = ask("Install which skills? Numbers separated by spaces, or `all`: ")?;
    if answer == "all" {
        return Ok(candidates.iter().collect());
    }
    let mut picked = BTreeSet::new();
    for word in answer
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
    {
        match word.parse::<usize>() {
            Ok(n) if (1..=candidates.len()).contains(&n) => {
                picked.insert(n - 1);
            }
            _ => {
                return Err(Failure::refused(format!(
                    "`{word}` is not one of the numbers 1 to {}; nothing was written",
                    candidates.len()
                )));
            }
        }
    }
    if picked.is_empty() {
        return Err(Failure::refused("no skill chosen; nothing was written"));
    }
    Ok(picked.into_iter().map(|i| &candidates[i]).collect())
}

/// Reads the upstream `SKILL.md` and decides the install name.
fn inspect(
    source: Source,
    snapshot: Snapshot,
    candidate: &Candidate,
    name: Option<String>,
    group: Option<&str>,
) -> Result<Install, Failure> {
    let label = if source.path.is_empty() {
        format!("SKILL.md in {}", source.url)
    } else {
        format!("{}/SKILL.md in {}", source.path, source.url)
    };
    let Some(file) = snapshot.file("SKILL.md") else {
        return Err(Failure::refused(format!("{label} is not a regular file")));
    };
    let skill_md = SkillMd::parse(&label, &file.content)?;
    let upstream_name = candidate.upstream_name.clone();
    let name = name.unwrap_or_else(|| upstream_name.clone());
    let renamed_skill_md = if skill_md.name() == Some(name.as_str()) {
        None
    } else {
        Some(skill_md.with_name(&name)?)
    };
    Ok(Install {
        source,
        snapshot,
        skill: skill_path(group, &name),
        name,
        upstream_name,
        renamed_skill_md,
    })
}

/// Refuses (exit 1) the whole add when any install name is invalid,
/// reserved or taken (also by another selected skill), pointing to `--name`.
/// What this add writes itself does not take a name: an add stopped
/// part-way finishes when it is run again.
fn check_install_names(
    home: &Path,
    repo: &SkillRepo,
    installs: &[Install],
    upstream: &Upstream,
) -> Result<(), Failure> {
    let mut seen = BTreeMap::new();
    for install in installs {
        let name = &install.name;
        if let Some(other) = seen.insert(name.as_str(), install.source.path.as_str()) {
            let path = &install.source.path;
            return Err(Failure::refused(format!(
                "the selected skills in {other}/ and {path}/ are both named `{name}`; nothing was installed; install each on its own with `--skill {other}/ --name <new name>` or `--skill {path}/ --name <new name>`"
            )));
        }
        let check = names::check_new_name(name).and_then(|()| {
            names::check_not_taken(home, repo, name, |taken| {
                ours(repo, install, upstream, taken)
            })
        });
        check.map_err(|failure| {
            let hint = if installs.len() == 1 {
                "install it under another name with `--name <name>`".to_owned()
            } else {
                let path = &install.source.path;
                format!("nothing was installed; install that skill on its own with `--skill {path}/ --name <new name>`")
            };
            Failure::refused(format!("{}; {hint}", failure.message))
        })?;
    }
    Ok(())
}

/// The full file list, with scripts and executables flagged.
fn print_file_list(install: &Install, upstream: &Upstream) {
    let upstream_name = if install.upstream_name == install.name {
        String::new()
    } else {
        format!(" (upstream name {})", install.upstream_name)
    };
    let path = if install.source.path.is_empty() {
        String::new()
    } else {
        format!(" ({})", install.source.path)
    };
    out!(
        "skill {}{upstream_name} from {}{path} {}:",
        install.name,
        install.source.url,
        upstream.version()
    );
    if let Some(r) = upstream.resolved()
        && r.also_tag
    {
        out!(
            "  note: `{}` is also a tag; the branch is installed",
            r.name
        );
    }
    for file in &install.snapshot.files {
        let flags = run_flags(&file.path, &file.content, file.executable);
        out!("  {}{flags}", file.path);
    }
    print_not_installed(&install.snapshot, "  ");
    if install.renamed_skill_md.is_some() {
        out!(
            "the `name` line of {1}/SKILL.md is set to `{0}` (a local modification; upstream/{0} keeps the upstream text)",
            install.name,
            install.skill
        );
    }
}

/// The source table and lock entry `add` writes for `install`.
fn records(install: &Install, upstream: &Upstream) -> (SourceRecord, LockEntry) {
    let resolved = upstream.resolved();
    let source = SourceRecord {
        url: install.source.url.clone(),
        path: install.source.path.clone(),
        git_ref: resolved.map(|r| r.name.clone()),
        ref_kind: resolved.map(|r| r.kind.as_str()),
    };
    (source, upstream.lock_entry(&install.snapshot))
}

/// The files of the skill folder: the snapshot, with the `SKILL.md` whose
/// `name` line is set when it had to be.
fn skill_files(install: &Install) -> Vec<SnapshotFile> {
    let mut files = install.snapshot.files.clone();
    if let Some(renamed) = &install.renamed_skill_md {
        let file = files.iter_mut().find(|f| f.path == "SKILL.md");
        file.expect("a skill has a SKILL.md").content = renamed.clone().into_bytes();
    }
    files
}

/// Whether `taken` is what this add writes for `install`, left by an add
/// that stopped part-way: the folders hold the snapshot (`upstream/<name>`
/// is written first, so the skill folder counts only with it), the source
/// table and lock entry are this add's.
fn ours(
    repo: &SkillRepo,
    install: &Install,
    upstream: &Upstream,
    taken: Taken,
) -> Result<bool, Failure> {
    let name = &install.name;
    let same = |rel: String, files| {
        Ok::<_, Failure>(change::held(repo.root(), &rel, &Want::Folder(files))? == Held::Same)
    };
    let snapshot = || same(format!("upstream/{name}"), install.snapshot.files.clone());
    let (source, entry) = records(install, upstream);
    Ok(match taken {
        Taken::Folder => same(install.skill.clone(), skill_files(install))? && snapshot()?,
        Taken::Snapshot => snapshot()?,
        Taken::Table => Manifest::load(repo)?.source(name) == Some(source),
        Taken::LockEntry => Lockfile::load(repo)?.entry(name) == Some(entry),
    })
}

/// For each skill `upstream/<name>` and its skill folder, then `skiman.toml`
/// with their source tables and, last, `skiman.lock` with their entries.
fn change(repo: &SkillRepo, installs: &[Install], upstream: &Upstream) -> Result<Change, Failure> {
    let refuse = |path: &Path, e: String| Failure::refused(format!("{}: {e}", path.display()));
    let (manifest_file, lock_file) = (repo.manifest_file(), repo.lockfile());
    let mut manifest = read_text(&manifest_file)?.unwrap_or_default();
    let mut lock = read_text(&lock_file)?.unwrap_or_default();
    let mut pieces = Vec::new();
    for install in installs {
        let name = &install.name;
        let snapshot = install.snapshot.files.clone();
        pieces.push((format!("upstream/{name}"), Want::Folder(snapshot)));
        pieces.push((install.skill.clone(), Want::Folder(skill_files(install))));
        let (source, entry) = records(install, upstream);
        manifest = manifest::with_source(&manifest, name, &source)
            .map_err(|e| refuse(&manifest_file, e))?;
        lock = lockfile::with_entry(&lock, name, &entry).map_err(|e| refuse(&lock_file, e))?;
    }
    pieces.push((
        rules::MANIFEST.to_owned(),
        Want::File(manifest.into_bytes()),
    ));
    pieces.push((rules::LOCKFILE.to_owned(), Want::File(lock.into_bytes())));
    Ok(Change {
        pieces,
        command: "add",
        save_files: false,
    })
}

/// The rules on everything `add` writes, before anything is written. The
/// candidate output is complete here: every file of both folders of every
/// selected skill is in `scope.files` (the skills also in `scope.skills`,
/// the snapshots in `scope.upstreams`), plus the new source tables and lock
/// entries, so the secret rule scans the complete candidate output.
fn validate(repo: &SkillRepo, installs: &[Install], change: &Change) -> Result<(), Failure> {
    let working_tree = WorkingTree::new(repo.root());
    let mut files = BTreeSet::new();
    for (rel, want) in &change.pieces {
        if let Want::Folder(folder) = want {
            files.extend(folder.iter().map(|f| format!("{rel}/{}", f.path)));
        }
    }
    let names: BTreeSet<String> = installs.iter().map(|i| i.name.clone()).collect();
    let scope = Scope {
        files,
        skills: names.clone(),
        upstreams: names.clone(),
        manifest_tables: names.clone(),
        lock_entries: names.clone(),
        lock: !repo.lockfile().exists(),
        consistency: names,
        ..Scope::default()
    };
    rules::validate_write(&change.overlay(&working_tree), &scope)
}
