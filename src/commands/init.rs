//! `skiman init <path>`: create or reuse a canon, write the user config,
//! then sync. When the user config pointed to another canon, that sync
//! treats skiman's links into it as owned. Before that sync it installs the
//! git hooks, and finally it adds or removes the session-start hook entries.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Exit;
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::commands::has_terminal;
use crate::config::UserConfig;
use crate::failure::{Failure, finish};
use crate::fsutil::absolute_lexical;
use crate::git_hooks;
use crate::linked::{self, Linked, gitignore};
use crate::manifest::Manifest;
use crate::manifest::new_manifest_text;
use crate::paths::Paths;
use crate::session_hooks;
use crate::sync::{self, ItemKind, Output};
use crate::test_hook;
use crate::tools::TOOLS;

/// `session_hooks` is the validated `--session-hooks` value (`none` or tool ids).
pub fn run(path: &Path, session_hooks: Option<&str>) -> Exit {
    finish(init(path, session_hooks))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// Missing or empty: `git init`, then create the layout.
    Create,
    /// Holds only `.git` (a fresh `git init` or a clone of an empty repo):
    /// create the layout in that repo.
    CreateInRepo,
    /// Holds `skiman.toml`: reuse it as it is.
    Reuse,
}

fn init(path: &Path, session_hooks: Option<&str>) -> Result<Exit, Failure> {
    if session_hooks.is_none() && !has_terminal() {
        return Err(Failure::usage(
            "missing flag without a terminal: --session-hooks <ids|none>",
        ));
    }
    let paths = Paths::from_env()?;
    let cwd = std::env::current_dir()
        .map_err(|e| Failure::operational(format!("cannot read the current directory: {e}")))?;
    let path = absolute_lexical(&cwd, path);
    test_hook::at("init", "locking");
    let machine = CanonLock::machine(&paths)?;
    // Every command that changes `skiman.toml` holds the machine lock while
    // it does, so the current canon's registrations read now still hold
    // when the canon is made.
    let config = UserConfig::load(&paths).ok();
    let current = config
        .as_ref()
        .and_then(|config| Canon::at(&config.repo).ok())
        .and_then(|current| {
            Linked::load(paths.home(), &current)
                .ok()
                .map(|l| (current, l))
        });
    // Where the canon lands, checked once and made there: never through a
    // link on `path` again, which may change meanwhile.
    let place = linked::written_through(&path);
    refuse_inside_linked(current.as_ref(), &path, &place)?;
    test_hook::at("init", "checked");
    let target = inspect(&place)?;

    if target == Target::Create {
        git_init(&place)?;
    }
    let repo = Canon::at(&place)?;
    let lock = CanonLock::of_init(machine, &repo)?;
    // The canon switched away from: its links are owned for the closing sync.
    let previous = config
        .map(|config| config.repo.canonicalize().unwrap_or(config.repo))
        .filter(|previous| previous != repo.root())
        .map(|root| sync::PreviousCanon { root });
    if target != Target::Reuse {
        write_layout(&repo, paths.home())
            .map_err(|e| Failure::operational(format!("cannot create the canon: {e}")))?;
        out!("created canon {}", path.display());
    }
    let guarded = ignore_lines(&repo, target)?;
    // The user config names `path`: when a link on it was re-pointed since
    // the check, it no longer leads to the canon made, so it is not saved.
    let now = linked::written_through(&path);
    if now != place {
        return Err(Failure::refused(format!(
            "{} now leads to {}, not to {} where the canon is; {} is not written, run `skiman init` again",
            path.display(),
            now.display(),
            place.display(),
            paths.config_file().display()
        )));
    }
    let binary = invoked_binary(&cwd);
    UserConfig {
        repo: path,
        binary: binary.clone(),
    }
    .save(&paths)?;
    install_git_hooks(&repo, &binary)?;
    let (exit, report) =
        sync::run_with(&paths, &repo, &lock, Output::Normal, previous.as_ref(), &[]);
    hint_not_owned(&report, &repo);
    // Settings files are outside the canon: do not hold the canon lock
    // while the preview waits for an answer; each write takes it again.
    drop(lock);
    let hooks = install_session_hooks(&paths, &repo, &binary, session_hooks);
    Ok(guarded.combine(exit).combine(hooks))
}

/// Refuses a canon at `path`, which lands at `target`
/// ([`linked::written_through`]), when `target` is, or lies inside, a linked
/// folder of the current canon or the target of one of its valid child
/// links: making it would write there. While a folder cannot be read its
/// child links are unknown, so any new canon is refused; the current canon
/// itself is reused. With no current canon whose `skiman.toml` can be
/// read, no folder is known.
fn refuse_inside_linked(
    current: Option<&(Canon, Linked)>,
    path: &Path,
    target: &Path,
) -> Result<(), Failure> {
    let Some((current, linked)) = current else {
        return Ok(());
    };
    if let Some(folder) = linked.unreadable()
        && target != current.root()
    {
        return Err(Failure::refused(format!(
            "linked folder `{}` cannot be read now, so skiman cannot tell whether {} lies where one of its links leads; nothing was changed, run `skiman sync` to see why",
            folder.id,
            path.display()
        )));
    }
    if let Some(folder) = linked.holding(target) {
        return Err(Failure::refused(format!(
            "{} is, or lies inside, linked folder `{id}` of the current canon, where skiman writes nothing; to make a canon there, first run `skiman linked remove {id}`",
            path.display(),
            id = folder.id
        )));
    }
    if let Some((folder, child)) = linked.led_to(target) {
        let place = if target == child {
            "is where".to_owned()
        } else {
            format!("lies inside {}, where", child.display())
        };
        return Err(Failure::refused(format!(
            "{} {place} a link in linked folder `{id}` of the current canon leads, so skiman writes nothing there; to make a canon there, first remove that link or run `skiman linked remove {id}`",
            path.display(),
            id = folder.id
        )));
    }
    Ok(())
}

/// `--session-hooks <ids>` adds entries, `none` removes skiman's, and no flag
/// (only possible with a terminal) previews the entries and asks. Each
/// settings file is written under the canon lock, and left alone when it
/// resolves into a linked folder registered at that moment.
fn install_session_hooks(
    paths: &Paths,
    repo: &Canon,
    binary: &Path,
    session_hooks: Option<&str>,
) -> Exit {
    let request = match session_hooks {
        Some("none") => session_hooks::Request::Remove,
        Some(ids) => session_hooks::Request::Add(ids),
        None => session_hooks::Request::Ask,
    };
    let left = |message: String| Failure {
        exit: Exit::NeedsAttention,
        message: format!("{message}; its session-start hook entry is left as it is"),
    };
    let guard = |file: &Path, lands: &Path| -> Result<CanonLock, Failure> {
        let lock = CanonLock::wait(paths, repo)?;
        let linked = Linked::read(paths.home(), repo, &Manifest::load(repo)?);
        match linked.refusal(file, lands) {
            Some(message) => Err(left(message)),
            None => Ok(lock),
        }
    };
    session_hooks::apply(paths.home(), binary, request, &guard)
}

/// Story 5: content skiman does not own in the tool directories or the
/// instruction locations. With no skills in the canon yet, the way in is
/// `skiman import`; otherwise `skiman adopt` for each foreign skill, and an
/// instruction file is moved away by hand (skiman never replaces it).
fn hint_not_owned(report: &sync::Report, repo: &Canon) {
    // A link into a linked folder is never adopted or imported, and nothing
    // is while a linked folder cannot be read.
    let foreign = report.items.iter().any(|item| {
        matches!(&item.kind, ItemKind::Foreign { into, .. } if matches!(into, sync::PointsInto::Elsewhere | sync::PointsInto::Registrable(_)))
    });
    let refused = report
        .items
        .iter()
        .any(|item| item.kind == ItemKind::RefusedLocation);
    if (foreign || refused) && !has_skills(repo) {
        err!(
            "hint: the canon has no skills yet and the tool directories or instruction locations hold content skiman does not own; bring your existing setup in with `skiman import` (see `skiman import --guide`)"
        );
        return;
    }
    if foreign {
        err!(
            "hint: the tool directories hold foreign skills; bring each one into the canon with `skiman adopt <name> --from <tool id>`"
        );
    }
    if refused {
        err!(
            "hint: move each instruction file listed above away by hand (copy what you want to keep into {} first), then run `skiman sync`",
            repo.shared_body().display()
        );
    }
}

/// Installs the git hooks and tells the user what to add by hand where
/// skiman left a hook alone.
fn install_git_hooks(repo: &Canon, binary: &Path) -> Result<(), Failure> {
    let installed = git_hooks::install(repo.root(), binary)?;
    if installed.not_a_git_repo {
        err!(
            "warning: {} is not in a git repository, so skiman installed no git hooks; run `git init` there, then `skiman init` again",
            repo.root().display()
        );
    }
    for path in &installed.written {
        out!("installed git hook {}", path.display());
    }
    for path in &installed.foreign {
        let name = path.file_name().expect("hook has a name").to_string_lossy();
        err!(
            "warning: {} is not skiman's git hook, so skiman left it unchanged; add this line to it:\n  {}",
            path.display(),
            git_hooks::line_to_add(&name, binary)
        );
    }
    if let Some((dir, lacking)) = &installed.hooks_path_elsewhere
        && !lacking.is_empty()
    {
        let what = match lacking.len() {
            1 => "this line to the hook",
            _ => "these lines to the hooks",
        };
        err!(
            "warning: core.hooksPath points to {}, so skiman installed no git hooks; add {what} there:",
            dir.display()
        );
        for name in lacking {
            err!("  {name}:\n    {}", git_hooks::line_to_add(name, binary));
        }
    }
    Ok(())
}

/// Whether any skill folder under `skills/` holds a `SKILL.md`.
fn has_skills(repo: &Canon) -> bool {
    repo.layout().is_ok_and(|layout| {
        layout
            .skills()
            .iter()
            .any(|placed| repo.root().join(placed.path()).join("SKILL.md").is_file())
    })
}

fn inspect(path: &Path) -> Result<Target, Failure> {
    let mut entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Target::Create),
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => {
            return Err(Failure::refused(format!(
                "{} is not a folder",
                path.display()
            )));
        }
        Err(e) => {
            return Err(Failure::operational(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    if std::fs::symlink_metadata(path.join("skiman.toml")).is_ok() {
        return Ok(Target::Reuse);
    }
    match entries.next() {
        None => Ok(Target::Create),
        Some(Ok(entry)) if entry.file_name() == ".git" && entries.next().is_none() => {
            Ok(Target::CreateInRepo)
        }
        Some(_) => Err(Failure::refused(format!(
            "{} is not empty and holds no skiman.toml; choose an empty or missing folder, a folder that holds only `.git` (a fresh `git init` or a clone of an empty repo), or an existing canon",
            path.display()
        ))),
    }
}

/// `git init` with no commit; creates `path` when missing.
fn git_init(path: &Path) -> Result<(), Failure> {
    let output = Command::new("git")
        .args(["init", "--quiet", "--"])
        .arg(path)
        .output()
        .map_err(|e| Failure::operational(format!("cannot run git (skiman requires git): {e}")))?;
    if !output.status.success() {
        return Err(Failure::operational(format!(
            "git init {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// `skills/`, an empty `instructions/shared.md`, and `skiman.toml` listing
/// the tools present right now (`init` adds the `.gitignore` lines next).
fn write_layout(repo: &Canon, home: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(repo.skills_dir())?;
    let shared = repo.shared_body();
    std::fs::create_dir_all(shared.parent().expect("shared body has a parent"))?;
    std::fs::write(&shared, "")?;
    let present: Vec<&str> = TOOLS
        .iter()
        .filter(|tool| tool.is_present(home))
        .map(|tool| tool.id)
        .collect();
    std::fs::write(repo.manifest_file(), new_manifest_text(&present))
}

/// Adds `.skiman/` and `/linked/` to the canon's `.gitignore` where git
/// does not ignore those folders yet ([`gitignore::ignore`]), saying so in
/// a reused canon. A `.gitignore` that is a link is not written through:
/// git does not read it, so a warning says to replace it or to use
/// `.git/info/exclude` (exit 3).
fn ignore_lines(repo: &Canon, target: Target) -> Result<Exit, Failure> {
    let path = repo.root().join(".gitignore");
    let mut not_added = Vec::new();
    for line in [gitignore::SKIMAN_LINE, gitignore::LINE] {
        match gitignore::ignore(repo.root(), line)? {
            gitignore::Outcome::Added if target == Target::Reuse => {
                out!("added {line} to {}", path.display());
            }
            gitignore::Outcome::Link => not_added.push(format!("`{line}`")),
            _ => {}
        }
    }
    if not_added.is_empty() {
        return Ok(Exit::Success);
    }
    err!(
        "warning: {} is a link, and git does not read a `.gitignore` that is a link, so skiman writes nothing to it; replace the link with a file holding its lines plus {}, or add those lines to .git/info/exclude",
        path.display(),
        not_added.join(", ")
    );
    Ok(Exit::NeedsAttention)
}

/// The path skiman was invoked through: `argv[0]` made absolute, or resolved
/// against `PATH` when it has no `/`. Symlinks are not resolved, so the hooks
/// keep working when a package manager swaps the binary behind the link.
fn invoked_binary(cwd: &Path) -> PathBuf {
    let current_exe = || std::env::current_exe().unwrap_or_default();
    let Some(argv0) = std::env::args_os().next() else {
        return current_exe();
    };
    if argv0.as_bytes().contains(&b'/') {
        return absolute_lexical(cwd, Path::new(&argv0));
    }
    find_on_path(&argv0)
        .map(|found| absolute_lexical(cwd, &found))
        .unwrap_or_else(current_exe)
}

fn find_on_path(program: &OsStr) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}
