//! Running `skiman import <folder>` again after it stopped part-way: every
//! run works the whole import out again from what is on disk, keeps what is
//! already in place and does the rest (ADR 0005).
//!
//! A precise step is hit with the test-only hook: `SKIMAN_TEST_HOOK` names
//! `import:<point>` and `SKIMAN_TEST_HOOK_RUN` is the shell command debug
//! builds run there; `kill -9 $PPID` kills the import. Points:
//! `rename:<repo path>` before each repo piece is renamed into place, and
//! `back-up:` and `moved:` + `<backup path>` before and after each original
//! moves to the backup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::support::{
    Sandbox, Setup, Tool, backup_dirs, import, import_killed_at, import_with_hook, only_backup,
    read, setup, skill_md, tree, write,
};

/// [`tree`] of `dir` with each path below `prefix` and the sandbox root
/// replaced, so two sandboxes compare equal.
fn normalised(sandbox: &Sandbox, prefix: &str, dir: &Path) -> BTreeMap<String, String> {
    let root = sandbox.root().display().to_string();
    tree(dir)
        .into_iter()
        .map(|(path, value)| {
            let key = Path::new(prefix).join(path).display().to_string();
            (key, value.replace(&root, "<root>"))
        })
        .collect()
}

/// The skill repo and the home folder (tool directories, instruction files,
/// machine state). Backups and the git cache are left out: a killed run
/// leaves a backup folder of its own, and fetching fills the cache.
fn final_state(sandbox: &Sandbox) -> BTreeMap<String, String> {
    let mut state = normalised(sandbox, "repo", &sandbox.skill_repo());
    state.extend(normalised(sandbox, "home", &sandbox.home()));
    state.retain(|key, _| {
        !key.starts_with("home/.local/state/skiman/backups")
            && !key.starts_with("home/.local/state/skiman/cache")
    });
    state
}

/// What an import that was never interrupted ends with.
struct Uninterrupted {
    state: BTreeMap<String, String>,
    /// Its one backup folder: every original and replaced repo file.
    backup: BTreeMap<String, String>,
}

/// Imports [`setup`] with `layout` added, without interruption.
fn uninterrupted(layout: fn(&Sandbox)) -> Uninterrupted {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    layout(&sandbox);
    let output = import(&sandbox, &folder);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Uninterrupted {
        state: final_state(&sandbox),
        backup: normalised(&sandbox, "", &only_backup(&sandbox)),
    }
}

/// Kills the import of [`setup`] with `layout` added at `point`, runs
/// `skiman sync` when `sync_first`, deletes every upstream (so a fetch
/// would fail) and runs the same command again. What differs from
/// `expected`, if anything: the exit code, the import folder, the final
/// state, or an original or replaced file that is in no backup folder.
fn killed_and_run_again(
    layout: fn(&Sandbox),
    point: &str,
    sync_first: bool,
    expected: &Uninterrupted,
) -> Option<String> {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    layout(&sandbox);
    let folder_before = tree(&folder);
    import_killed_at(&sandbox, &folder, point);
    if sync_first {
        sandbox.skiman().arg("sync").output().unwrap();
    }
    std::fs::remove_dir_all(sandbox.upstreams_dir()).unwrap();

    let output = import(&sandbox, &folder);
    let mut problems = Vec::new();
    if output.status.code() != Some(0) {
        problems.push(format!(
            "exit {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if tree(&folder) != folder_before {
        problems.push("the import folder changed".to_owned());
    }
    let state = final_state(&sandbox);
    for (key, value) in &expected.state {
        if state.get(key) != Some(value) {
            problems.push(format!(
                "{key}: expected {value:?}, found {:?}",
                state.get(key)
            ));
        }
    }
    for key in state.keys().filter(|k| !expected.state.contains_key(*k)) {
        problems.push(format!("{key}: not expected, found {:?}", state[key]));
    }
    let backups: Vec<_> = backup_dirs(&sandbox)
        .iter()
        .map(|dir| normalised(&sandbox, "", dir))
        .collect();
    for (path, value) in &expected.backup {
        if !backups.iter().any(|b| b.get(path) == Some(value)) {
            problems.push(format!("{path} ({value:?}) is in no backup"));
        }
    }
    let then = if sync_first { ", then sync" } else { "" };
    (!problems.is_empty()).then(|| format!("kill at {point}{then}:\n  {}", problems.join("\n  ")))
}

/// Every pair of `points` and `sync_first` ends like an uninterrupted import.
#[track_caller]
fn assert_each_kill_ends_like_an_uninterrupted_import(
    layout: fn(&Sandbox),
    points: &[String],
    sync_first: &[bool],
) {
    let expected = uninterrupted(layout);
    let failures: Vec<String> = points
        .iter()
        .flat_map(|point| sync_first.iter().map(move |sync| (point, *sync)))
        .filter_map(|(point, sync)| killed_and_run_again(layout, point, sync, &expected))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The kill points of [`setup`]: before each repo piece is renamed into
/// place, and before and after each original moves.
fn standard_points() -> Vec<String> {
    let renames = [
        "upstream/pdf",
        "skiman.lock",
        "skiman.toml",
        "instructions/shared.md",
        "skills/notes",
        "skills/pdf",
    ];
    let originals = [
        "claude-code/old-copy",
        "codex/notes",
        "claude-code/notes",
        "claude-code/CLAUDE.md",
        "codex/AGENTS.md",
    ];
    renames
        .iter()
        .map(|rel| format!("rename:{rel}"))
        .chain(
            originals
                .iter()
                .flat_map(|o| [format!("back-up:{o}"), format!("moved:{o}")]),
        )
        .collect()
}

/// After each original of a layout moves.
fn moved(originals: &[&str]) -> Vec<String> {
    originals.iter().map(|o| format!("moved:{o}")).collect()
}

fn standard(_: &Sandbox) {}

/// Adds `~/.gemini/config/skills/notes -> ~/.agents/skills/notes`, so three
/// links lead to the copy in `~/.claude/skills`.
fn chain(sandbox: &Sandbox) {
    std::os::unix::fs::symlink(
        sandbox.tool_dir(Tool::Codex).join("notes"),
        sandbox.tool_dir(Tool::Antigravity).join("notes"),
    )
    .unwrap();
}

/// `~/.claude/skills/alias -> ~/.claude/skills/notes`, and the Codex link
/// leads to the copy through it.
fn alias(sandbox: &Sandbox) {
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    std::os::unix::fs::symlink(claude.join("notes"), claude.join("alias")).unwrap();
    let codex = sandbox.tool_dir(Tool::Codex).join("notes");
    std::fs::remove_file(&codex).unwrap();
    std::os::unix::fs::symlink(claude.join("alias"), codex).unwrap();
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_kill_at_any_step_then_the_same_command_ends_like_an_uninterrupted_import() {
    assert_each_kill_ends_like_an_uninterrupted_import(standard, &standard_points(), &[false]);
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_kill_at_any_step_then_sync_then_the_same_command_ends_like_an_uninterrupted_import() {
    assert_each_kill_ends_like_an_uninterrupted_import(standard, &standard_points(), &[true]);
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn with_three_links_in_a_chain_a_kill_after_any_move_then_the_same_command_ends_like_an_uninterrupted_import()
 {
    let originals = [
        "claude-code/old-copy",
        "antigravity/notes",
        "codex/notes",
        "claude-code/notes",
        "claude-code/CLAUDE.md",
        "codex/AGENTS.md",
    ];
    assert_each_kill_ends_like_an_uninterrupted_import(chain, &moved(&originals), &[false, true]);
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn with_a_link_through_an_alias_a_kill_after_any_move_then_the_same_command_ends_like_an_uninterrupted_import()
 {
    let originals = [
        "claude-code/old-copy",
        "codex/notes",
        "claude-code/alias",
        "claude-code/notes",
        "claude-code/CLAUDE.md",
        "codex/AGENTS.md",
    ];
    assert_each_kill_ends_like_an_uninterrupted_import(alias, &moved(&originals), &[false, true]);
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_kill_while_skiman_toml_is_replaced_leaves_the_old_one_and_running_again_keeps_the_project_roots()
 {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    write(sandbox.home().join("proj/AGENTS.md"), "# Proj\n");
    sandbox
        .skiman()
        .args(["project", "add"])
        .arg(sandbox.home().join("proj"))
        .assert()
        .code(0);
    let repo = sandbox.skill_repo();
    let manifest = read(repo.join("skiman.toml"));

    import_killed_at(&sandbox, &folder, "rename:skiman.toml");
    assert_eq!(read(repo.join("skiman.toml")), manifest);
    // A session start in between finds a skill repo it can read.
    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .stderr_eq("");

    let output = import(&sandbox, &folder);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = read(repo.join("skiman.toml"));
    assert!(
        manifest.contains("[projects.proj]\npath = \"~/proj\"\n"),
        "{manifest}"
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_file_written_where_sync_is_about_to_link_is_kept_and_the_same_command_moves_it_to_the_backup()
{
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    let claude_md = sandbox.instruction_file(Tool::ClaudeCode);
    // A session writes the instruction file after the takeover moved the old one.
    let output = import_with_hook(
        &sandbox,
        &folder,
        &format!("sync:link:{}", claude_md.display()),
        &format!(
            "echo 'a rule written meanwhile' > '{}'",
            claude_md.display()
        ),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert_eq!(read(&claude_md), "a rule written meanwhile\n");

    let output = import(&sandbox, &folder);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let shared = sandbox.skill_repo().join("instructions/shared.md");
    crate::support::assert_symlink(&claude_md, &shared, &shared);
    assert!(
        backup_dirs(&sandbox).iter().any(|dir| {
            dir.join("claude-code/CLAUDE.md").is_file()
                && read(dir.join("claude-code/CLAUDE.md")) == "a rule written meanwhile\n"
        }),
        "the file written meanwhile is in no backup"
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_branch_that_moved_after_a_kill_before_the_last_skill_is_fetched_again_and_the_old_snapshot_kept_in_a_backup()
 {
    let sandbox = Sandbox::new();
    let Setup {
        folder,
        upstream,
        commit,
    } = setup(&sandbox);
    let manifest = folder.join("skiman.toml");
    write(&manifest, read(&manifest).replace(&commit, "main"));
    import_killed_at(&sandbox, &folder, "rename:skills/notes");
    upstream.write("skills/pdf/reference.md", "# Reference v2\n");
    let v2 = upstream.commit("v2");

    let output = import(&sandbox, &folder);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    let repo = sandbox.skill_repo();
    assert!(
        stdout.contains(&format!(
            "  move {} to the backup, then write the import's\n",
            repo.join("upstream/pdf").display()
        )),
        "{stdout}"
    );
    assert_eq!(
        read(repo.join("upstream/pdf/reference.md")),
        "# Reference v2\n"
    );
    assert!(read(repo.join("skiman.lock")).contains(&format!("commit = \"{v2}\"")));
    assert!(
        backup_dirs(&sandbox).iter().any(|dir| {
            dir.join("repo/upstream/pdf/reference.md").is_file()
                && read(dir.join("repo/upstream/pdf/reference.md")) == "# Reference\n"
        }),
        "the old snapshot is in no backup"
    );
}

/// The skill repo, the home folder (tool directories, instruction files,
/// machine state without the git cache) and the import folder, to prove a
/// command changed nothing.
fn world(sandbox: &Sandbox) -> Vec<BTreeMap<PathBuf, String>> {
    let mut home = tree(sandbox.home());
    home.retain(|path, _| !path.starts_with(".local/state/skiman/cache"));
    vec![
        tree(sandbox.skill_repo()),
        home,
        tree(sandbox.root().join("import")),
    ]
}

/// Runs the import and asserts it refuses (exit 1) with every text in
/// `stderr_has`, changing nothing.
#[track_caller]
fn assert_run_refused(sandbox: &Sandbox, folder: &Path, stderr_has: &[&str]) -> String {
    let before = world(sandbox);
    let output = import(sandbox, folder);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    for text in stderr_has {
        assert!(stderr.contains(text), "{text:?} not in {stderr}");
    }
    assert_eq!(world(sandbox), before, "{stderr}");
    stderr
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn repo_content_changed_by_hand_before_the_last_skill_is_written_is_replaced_and_a_changed_skill_refuses_the_run()
 {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    let repo = sandbox.skill_repo();
    // upstream/pdf, skiman.lock, skiman.toml and shared.md are written;
    // skills/notes is next.
    import_killed_at(&sandbox, &folder, "rename:skills/notes");

    let manifest = read(repo.join("skiman.toml"));
    write(repo.join("skiman.toml"), format!("{manifest}# hand edit\n"));
    write(repo.join("upstream/pdf/extra.md"), "hand edit\n");
    write(repo.join("skills/mine/SKILL.md"), skill_md("mine"));
    write(
        repo.join("skills/notes/SKILL.md"),
        format!("{}\nMade by hand.\n", skill_md("notes")),
    );
    let stderr = assert_run_refused(
        &sandbox,
        &folder,
        &[
            "  skills/notes differs from the import folder's skills/notes\n",
            "  skills/mine is not in the import folder\n",
        ],
    );
    assert!(!stderr.contains("upstream/pdf"), "{stderr}");
    assert!(!stderr.contains("skiman.toml"), "{stderr}");

    // Without the two skills, the run replaces the rest, keeping each in a backup.
    std::fs::remove_dir_all(repo.join("skills/mine")).unwrap();
    std::fs::remove_dir_all(repo.join("skills/notes")).unwrap();
    let output = import(&sandbox, &folder);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(read(repo.join("skiman.toml")), manifest);
    assert!(!repo.join("upstream/pdf/extra.md").exists());
    assert_eq!(read(repo.join("skills/notes/SKILL.md")), skill_md("notes"));
    let backup = backup_dirs(&sandbox)
        .into_iter()
        .find(|dir| dir.join("repo/upstream/pdf/extra.md").exists())
        .expect("a backup holds the old snapshot");
    assert_eq!(
        read(backup.join("repo/skiman.toml")),
        format!("{manifest}# hand edit\n")
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn originals_backed_up_before_the_kill_are_still_in_their_backup_after_the_same_command_finishes() {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    import_killed_at(&sandbox, &folder, "moved:claude-code/notes");
    let first = only_backup(&sandbox);
    let backed_up = tree(&first);
    assert_eq!(
        read(first.join("claude-code/notes/SKILL.md")),
        skill_md("notes")
    );
    assert_eq!(
        read(first.join("repo/instructions/shared.md")),
        "old shared body\n"
    );

    let output = import(&sandbox, &folder);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    let second = stdout
        .lines()
        .find_map(|l| {
            l.strip_prefix("imported ")
                .and_then(|l| l.split_once("; the originals this run moved are in "))
        })
        .map(|(_, b)| PathBuf::from(b))
        .unwrap_or_else(|| panic!("no backup line in {stdout}"));
    assert_ne!(second, first);
    assert_eq!(tree(&first), backed_up);
    assert_eq!(
        read(second.join("claude-code/CLAUDE.md")),
        "old claude rules\n"
    );
    crate::support::assert_symlink(
        claude.join("notes"),
        sandbox.skill_repo().join("skills/notes"),
        sandbox.skill_repo().join("skills/notes"),
    );
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn after_a_kill_importing_another_folder_is_refused_naming_what_the_repo_holds_that_it_does_not() {
    let sandbox = Sandbox::new();
    let Setup { folder, .. } = setup(&sandbox);
    import_killed_at(&sandbox, &folder, "rename:skills/notes");
    let other = sandbox.root().join("other");
    write(other.join("skills/todo/SKILL.md"), skill_md("todo"));
    write(other.join("instructions/shared.md"), "Other.\n");
    write(other.join("skiman.toml"), "");

    assert_run_refused(
        &sandbox,
        &other,
        &[
            "  upstream/pdf is the snapshot of no third-party skill in the import folder\n",
            "  skiman.lock has an entry for pdf, which is no third-party skill in the import folder\n",
        ],
    );
}
