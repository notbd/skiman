//! `skiman check [--staged]`: report every rule violation in the working tree
//! of the skill repo, or in its staged snapshot, on stdout, and warnings on
//! stderr; on the working tree, also each skill that lacks a summary, group
//! or origin (a warning). When the staged snapshot breaks a rule that the working tree
//! passes, a hint says so and prints a line that stages the working tree.
//! Read-only; takes no lock.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Exit;
use crate::failure::{Failure, finish};
use crate::git_hooks::shell_quote;
use crate::paths::Paths;
use crate::repo::SkillRepo;
use crate::rules;
use crate::staged::{self, StagedTree};
use crate::tree::{Tree, WorkingTree};

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
    let report = rules::check_repo(tree.as_ref()).map_err(io)?;
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
    }
    if staged
        && !report.violations.is_empty()
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
