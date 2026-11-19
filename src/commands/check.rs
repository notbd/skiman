//! `skiman check [--staged]`: report every rule violation in the working tree
//! of the skill repo, or in its staged snapshot, on stdout, and warnings on
//! stderr; on the working tree, also each skill that lacks a summary, a group
//! folder or an origin, and what is wrong with linked skills (warnings). When the staged snapshot breaks a rule
//! that the working tree passes, a hint says so and prints a line that
//! stages the working tree.
//! Read-only; takes no lock.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Exit;
use crate::failure::{Failure, finish};
use crate::git_hooks::shell_quote;
use crate::linked::{self, LINKED_DIR, Linked, gitignore};
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::rules;
use crate::staged::{self, StagedTree};
use crate::tree::{Kind, Tree, WorkingTree};

pub fn run(staged: bool) -> Exit {
    finish(check(staged))
}

fn check(staged: bool) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = SkillRepo::from_config(&paths)?;
    if staged && let Some(other) = hook_of_another_repo(&repo) {
        out!(
            "skiman: {} is not the skill repo ({}), so its skiman pre-commit hook checks nothing",
            other.display(),
            repo.root().display()
        );
        return Ok(Exit::Success);
    }
    let tree: Box<dyn Tree> = if staged {
        Box::new(StagedTree::read(repo.root()).map_err(|e| {
            Failure::operational(format!(
                "cannot read the staged snapshot of {}: {e}",
                repo.root().display()
            ))
        })?)
    } else {
        Box::new(WorkingTree::new(repo.root()))
    };
    let io = |e| Failure::operational(format!("cannot read {}: {e}", repo.root().display()));
    let mut report = rules::check_repo(tree.as_ref()).map_err(io)?;
    let tracked = linked_in_index(tree.as_ref(), staged, repo.root());
    report.violations.extend(tracked.iter().cloned());
    report.violations.sort();
    for violation in &report.violations {
        out!("{violation}");
    }
    for unused in &report.unused_allows {
        out!("{unused}");
    }
    for warning in rules::warnings_all(tree.as_ref()).map_err(io)? {
        err!("warning: {warning}");
    }
    // Not on the staged snapshot: the pre-commit hook would print one line
    // per skill on every commit.
    if !staged {
        for gap in rules::missing_info(tree.as_ref()).map_err(io)? {
            err!("warning: {gap}");
        }
        for warning in linked_warnings(&paths, &repo)? {
            err!("warning: {warning}");
        }
        if let Some(warning) = linked_not_ignored(&repo) {
            err!("warning: {warning}");
        }
    }
    // Staging the working tree cannot take a path out of the index.
    if staged
        && !report.violations.is_empty()
        && tracked.is_empty()
        && let Some(hint) = stage_hint(repo.root())
    {
        out!("\n{hint}");
    }
    // Last: everything after its first line is pasted into skiman.toml.
    if let Some(hint) = rules::allow_hint(&report.violations) {
        out!("\n{hint}");
    }
    Ok(rules::exit_of(&report.violations))
}

/// What the linked skills on this machine warrant a warning for; nothing
/// when `skiman.toml` does not parse (that is a violation already).
fn linked_warnings(paths: &Paths, repo: &SkillRepo) -> Result<Vec<String>, Failure> {
    let Ok(manifest) = Manifest::load(repo) else {
        return Ok(Vec::new());
    };
    let linked = Linked::read(paths.home(), repo, &manifest);
    Ok(linked::check::warnings(&linked, &manifest, &repo.layout()?))
}

/// While `[linked]` tables exist in a canon that is a git repo: that git
/// does not ignore `linked/`, and where to add the line.
fn linked_not_ignored(repo: &SkillRepo) -> Option<String> {
    if Manifest::load(repo).ok()?.linked().is_empty()
        || gitignore::check_ignore(repo.root()) != Some(false)
    {
        return None;
    }
    let path = repo.root().join(".gitignore");
    let fix = match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_symlink() => format!(
            "{} is a link, and git does not read a `.gitignore` that is a link: replace it with a file that holds the line `{}`",
            path.display(),
            gitignore::LINE
        ),
        _ => format!("add the line `{}` to {}", gitignore::LINE, path.display()),
    };
    Some(format!(
        "git does not ignore `linked/`, so the links in it could be committed; {fix}"
    ))
}

/// `linked` and every path under it in the git index: the staged snapshot
/// when `staged`, else the repo's own index, never the folder on disk (it
/// holds links to content outside git). A canon that is not a git repo has
/// no index entries.
fn linked_in_index(tree: &dyn Tree, staged: bool, root: &Path) -> Vec<rules::Violation> {
    let paths = if staged {
        match tree.kind(LINKED_DIR) {
            Ok(Some(Kind::Dir)) => tree
                .walk(LINKED_DIR)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.kind != Kind::Dir)
                .map(|e| e.path)
                .collect(),
            Ok(Some(_)) => vec![LINKED_DIR.to_owned()],
            _ => Vec::new(),
        }
    } else {
        staged::paths_in_real_index(root, LINKED_DIR)
    };
    paths
        .into_iter()
        .map(|path| {
            rules::Violation::new(
                path,
                None,
                rules::rule::LINKED_TRACKED,
                "nothing under `linked/` may be in the git index; run `git rm -r --cached linked`",
            )
        })
        .collect()
}

/// After violations in the staged snapshot, when the working tree passes
/// every rule: the hint that the report is about the index, with a line
/// that stages each top-level path the rules read that is on disk or in the
/// repo's own index (git refuses a path that matches nothing). `None` when
/// the working tree breaks a rule too, or it or the index cannot be read.
fn stage_hint(root: &Path) -> Option<String> {
    if !rules::check_all(&WorkingTree::new(root)).ok()?.is_empty() {
        return None;
    }
    let indexed = staged::in_real_index(root, &rules::TOP_LEVEL_PATHS)?;
    let paths: Vec<&str> = rules::TOP_LEVEL_PATHS
        .into_iter()
        .filter(|p| indexed.contains(p) || root.join(p).symlink_metadata().is_ok())
        .collect();
    Some(hint_text(root, &paths.join(" ")))
}

/// The text of [`stage_hint`] for `paths` (separated by spaces). The root
/// is quoted for the shell, so its lines work from any directory.
fn hint_text(root: &Path, paths: &str) -> String {
    let git = format!("git -C {}", shell_quote(&root.to_string_lossy()));
    format!(
        "The report above is about the staged snapshot (the git index), not about your files: \
         the working tree passes every rule. Stage these paths with this line, then commit again:\n\
         \n  {git} add -A -- {paths}\n\
         \nIt stages every change under these paths; to commit only some of them, stage those \
         yourself. If the commit is still rejected, read the new report. Common causes:\n  \
         - an ignore rule hides a file: see `{git} status --ignored -- {paths}`\n  \
         - a skill folder holds its own .git, so git records it as a single entry of mode \
         160000: see `{git} ls-files -s -- {paths}`\n  \
         - line-ending conversion or a filter changed the content (core.autocrlf, .gitattributes)"
    )
}

/// Run from a git hook (`git commit` sets `GIT_INDEX_FILE`) of a repository
/// that is not the skill repo, e.g. a previous skill repo that still has
/// skiman's hooks: that repository's top level.
fn hook_of_another_repo(repo: &SkillRepo) -> Option<PathBuf> {
    std::env::var_os("GIT_INDEX_FILE")?;
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let top = PathBuf::from(
        String::from_utf8(output.stdout)
            .ok()?
            .trim_end_matches('\n'),
    );
    let top = top.canonicalize().unwrap_or(top);
    (top != repo.root()).then_some(top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_printed_line_gives_git_a_root_with_spaces_and_quotes_unchanged() {
        let root = Path::new("/tmp/skill repo/it's \"mine\"");
        let hint = hint_text(root, "skiman.toml skills");
        let line = hint
            .lines()
            .map(str::trim)
            .find(|l| l.contains(" add -A -- "))
            .expect("the hint has the line");

        // A shell function named `git` prints the arguments the shell parsed.
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("git() {{ printf '%s\\n' \"$@\"; }}; {line}"))
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let args: Vec<&str> = stdout.lines().collect();
        assert_eq!(args[..2], ["-C", root.to_str().unwrap()], "{line}");
        assert_eq!(args[2..], ["add", "-A", "--", "skiman.toml", "skills"]);
    }
}
