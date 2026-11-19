//! Every project write command killed at each of its writes, then run
//! again: the run again ends where a run never killed does (ADR 0005).

use std::path::PathBuf;

use crate::support::{Sandbox, assert_each_kill_finishes, init, skill_md, write};

/// The items of `~/work`: an instruction file at the root and in a folder,
/// and a project skill.
const ITEMS: [&str; 3] = ["CLAUDE.md", "a/AGENTS.md", ".claude/skills/foo"];

/// `~/work` registered with [`ITEMS`] in it, adopted when `adopted`.
fn work(sandbox: &Sandbox, adopted: bool) -> PathBuf {
    init(sandbox);
    let root = sandbox.home().join("work");
    write(root.join("CLAUDE.md"), "root v1\n");
    write(root.join("a/AGENTS.md"), "a v1\n");
    write(root.join(".claude/skills/foo/SKILL.md"), skill_md("foo"));
    let skiman = |args: &[&str]| sandbox.skiman().args(args).assert().code(0);
    skiman(&["project", "add", root.to_str().unwrap()]);
    if adopted {
        skiman(&["project", "adopt", "--yes"]);
    }
    root
}

/// Writes `text` into the item `rel` below `dir`: the file, or the
/// skill's `SKILL.md`.
fn edit(dir: PathBuf, rel: &str, text: &str) {
    match rel.ends_with(".md") {
        true => write(dir.join(rel), text),
        false => write(
            dir.join(rel).join("SKILL.md"),
            format!("{}{text}", skill_md("foo")),
        ),
    }
}

/// `project:write:<prefix><item>` for each of [`ITEMS`].
fn writes(prefix: &str) -> Vec<String> {
    ITEMS
        .iter()
        .map(|item| format!("project:write:{prefix}{item}"))
        .collect()
}

/// The arguments `args`, then the path of each of [`ITEMS`] in `root`.
fn naming_each(args: &[&str], root: &std::path::Path) -> Vec<String> {
    let paths = ITEMS
        .iter()
        .map(|item| root.join(item).display().to_string());
    args.iter().map(|a| a.to_string()).chain(paths).collect()
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_project_adopt_killed_before_each_write_finishes_when_run_again() {
    let hooks = writes("projects/work/");
    let hooks: Vec<&str> = hooks.iter().map(String::as_str).collect();
    assert_each_kill_finishes(
        |sandbox| {
            work(sandbox, false);
            vec!["project".into(), "adopt".into(), "--yes".into()]
        },
        &hooks,
    );
}

/// Each item changed on both sides, adopted by its exact path: the
/// project's version replaces the mirror, which goes to the backup.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_project_adopt_that_replaces_mirrors_killed_before_each_write_finishes_when_run_again() {
    let hooks = writes("projects/work/");
    let hooks: Vec<&str> = hooks.iter().map(String::as_str).collect();
    assert_each_kill_finishes(
        |sandbox| {
            let root = work(sandbox, true);
            for item in ITEMS {
                edit(root.clone(), item, "project v2\n");
                edit(sandbox.canon().join("projects/work"), item, "mirror v2\n");
            }
            naming_each(&["project", "adopt", "--yes"], &root)
        },
        &hooks,
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_project_restore_killed_before_each_write_finishes_when_run_again() {
    let hooks = writes("");
    let hooks: Vec<&str> = hooks.iter().map(String::as_str).collect();
    assert_each_kill_finishes(
        |sandbox| {
            let root = work(sandbox, true);
            for item in ITEMS {
                edit(sandbox.canon().join("projects/work"), item, "mirror v2\n");
            }
            naming_each(&["project", "restore", "--yes"], &root)
        },
        &hooks,
    );
}

/// Killed before the new `skiman.toml` is written, the run again finishes.
/// (Killed after it, before the synced hashes are saved, the root is no
/// longer registered: the run again is refused, and the next sync forgets
/// the hashes.)
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_project_eject_killed_before_each_write_finishes_when_run_again() {
    let mut hooks: Vec<String> = ITEMS
        .iter()
        .map(|item| format!("project:eject:projects/work/{item}"))
        .collect();
    hooks.push("project:write:skiman.toml".to_owned());
    let hooks: Vec<&str> = hooks.iter().map(String::as_str).collect();
    assert_each_kill_finishes(
        |sandbox| {
            let root = work(sandbox, true);
            vec![
                "project".into(),
                "eject".into(),
                "--yes".into(),
                root.display().to_string(),
            ]
        },
        &hooks,
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_sync_that_mirrors_project_edits_killed_before_each_write_finishes_when_run_again() {
    let hooks = writes("projects/work/");
    let hooks: Vec<&str> = hooks.iter().map(String::as_str).collect();
    assert_each_kill_finishes(
        |sandbox| {
            let root = work(sandbox, true);
            for item in ITEMS {
                edit(root.clone(), item, "project v2\n");
            }
            vec!["sync".into()]
        },
        &hooks,
    );
}

/// `project add` has one write, `skiman.toml` in one rename: killed before it,
/// nothing changed. (Killed after it, the root is registered and the run
/// again is refused: `skiman.toml` already has `[projects.work]`.)
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_project_add_killed_before_it_writes_finishes_when_run_again() {
    assert_each_kill_finishes(
        |sandbox| {
            init(sandbox);
            let root = sandbox.home().join("work");
            std::fs::create_dir_all(&root).unwrap();
            vec!["project".into(), "add".into(), root.display().to_string()]
        },
        &["project:write:skiman.toml"],
    );
}
