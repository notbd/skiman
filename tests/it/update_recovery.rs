//! `skiman update` failure paths (spec Merge semantics; ADR 0005): a change
//! between the merge and the repo lock, or while the update writes; a write
//! that fails or is killed part-way, which the same command finishes when
//! it runs again; an unwritable backups directory.
//!
//! A precise step is hit with the test-only hook: `SKIMAN_TEST_HOOK` names
//! `<skill>:<point>` and `SKIMAN_TEST_HOOK_RUN` is the shell command debug
//! builds run there.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::support::{
    Sandbox, Upstream, assert_each_kill_finishes, backup_dirs, backups, final_state, init, lines,
    read, tree, write,
};

/// Upstream `acme/<name>` (one skill at its root), installed as `<name>`.
fn installed(sandbox: &Sandbox, name: &str) -> Upstream {
    let up = sandbox.upstream(&format!("acme/{name}"));
    up.write(
        "SKILL.md",
        format!("---\nname: {name}\ndescription: The {name} skill\n---\n"),
    )
    .write("reference.md", lines(&[]))
    .write("forms.md", lines(&[]));
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", &format!("acme/{name}"), "--yes"])
        .assert()
        .code(0);
    up
}

/// `pdf` installed, with an uncommitted local edit, and a new upstream
/// version that merges cleanly with it.
fn pdf_with_an_update(sandbox: &Sandbox) -> Upstream {
    init(sandbox);
    let up = installed(sandbox, "pdf");
    write(
        sandbox.skill_repo().join("skills/pdf/forms.md"),
        lines(&[(1, "uncommitted edit")]),
    );
    up.write("reference.md", lines(&[(20, "upstream fix")]));
    up.commit("v2");
    up
}

/// `skiman update <names>` with the test hook set to run `command` at
/// `<skill>:<point>`.
fn update_with_hook(
    sandbox: &Sandbox,
    names: &[&str],
    at: &str,
    command: &str,
) -> snapbox::cmd::OutputAssert {
    sandbox
        .skiman()
        .arg("update")
        .args(names)
        .env("SKIMAN_TEST_HOOK", at)
        .env("SKIMAN_TEST_HOOK_RUN", command)
        .assert()
}

/// update fetches and merges without the repo lock: another command run
/// meanwhile goes through, and the merge leaves no folder behind.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn another_command_runs_while_update_fetches_and_merges() {
    let sandbox = Sandbox::new();
    let up = pdf_with_an_update(&sandbox);
    let repo = sandbox.skill_repo();
    let skiman = env!("CARGO_BIN_EXE_skiman");

    update_with_hook(
        &sandbox,
        &["pdf"],
        "pdf:prepared",
        &format!("'{skiman}' new notes -d 'Takes notes'"),
    )
    .code(0);
    assert!(repo.join("skills/notes/SKILL.md").is_file());
    assert_eq!(
        read(repo.join("skills/pdf/reference.md")),
        lines(&[(20, "upstream fix")])
    );
    assert!(read(repo.join("skiman.lock")).contains(&up.head()));
    assert_eq!(std::fs::read_dir(sandbox.tmp_dir()).unwrap().count(), 0);
}

/// Under the repo lock, update checks what it decided from: a change to
/// `skills/<name>` or the skill's records after the merge refuses the skill
/// (exit 1), and nothing of it changes. Another update of the same skill
/// meanwhile changes its lock entry: its result stays. `upstream/<name>` is
/// not what the merge reads: the lock entry gives the base, so an
/// `upstream/<name>` changed meanwhile, even while the update fetches, is
/// replaced, its content kept in the backup.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_change_after_the_merge_refuses_the_skill_under_the_repo_lock() {
    let changed = "skills/pdf, upstream/pdf or their records changed after the update read them; nothing of pdf changed, run the update again";
    for (file, line) in [
        ("skills/pdf/forms.md", "an edit\n"),
        ("skiman.toml", "ref = \"main\"\n"),
    ] {
        let sandbox = Sandbox::new();
        let up = pdf_with_an_update(&sandbox);
        let repo = sandbox.skill_repo();
        let lock = read(repo.join("skiman.lock"));

        let assert = update_with_hook(
            &sandbox,
            &["pdf"],
            "pdf:prepared",
            &format!("printf '{line}' >> '{}'", repo.join(file).display()),
        )
        .code(1)
        .stdout_eq("");
        assert_eq!(stderr_of(&assert), format!("error: skill pdf: {changed}\n"));
        assert!(read(repo.join(file)).ends_with(line), "{file}");
        assert_eq!(read(repo.join("skills/pdf/reference.md")), lines(&[]));
        assert_eq!(read(repo.join("skiman.lock")), lock);
        assert!(!lock.contains(&up.head()));
        assert!(backup_dirs(&sandbox).is_empty(), "{file}");
    }

    let sandbox = Sandbox::new();
    let up = pdf_with_an_update(&sandbox);
    let repo = sandbox.skill_repo();
    let forms = repo.join("upstream/pdf/forms.md");
    update_with_hook(
        &sandbox,
        &["pdf"],
        "pdf:prepared",
        &format!("printf 'an edit\\n' >> '{}'", forms.display()),
    )
    .code(0);
    assert_eq!(read(&forms), lines(&[]));
    assert!(read(repo.join("skiman.lock")).contains(&up.head()));
    let backup = backup_dirs(&sandbox).pop().unwrap();
    assert!(read(backup.join("repo/upstream/pdf/forms.md")).ends_with("an edit\n"));

    let sandbox = Sandbox::new();
    let up = pdf_with_an_update(&sandbox);
    let skiman = env!("CARGO_BIN_EXE_skiman");
    let assert = update_with_hook(
        &sandbox,
        &["pdf"],
        "pdf:prepared",
        &format!("SKIMAN_TEST_HOOK= '{skiman}' update pdf > /dev/null 2>&1"),
    )
    .code(1)
    .stdout_eq("");
    assert_eq!(stderr_of(&assert), format!("error: skill pdf: {changed}\n"));
    assert!(read(sandbox.skill_repo().join("skiman.lock")).contains(&up.head()));
    sandbox.skiman().arg("check").assert().code(0);

    let sandbox = Sandbox::new();
    init(&sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write(
        "SKILL.md",
        "---\nname: pdf\ndescription: The pdf skill\n---\n",
    )
    .write("forms.md", lines(&[]));
    up.commit("v1");
    // An ssh that runs the remote command here, first writing `$EDIT` when
    // it is set: `upstream/pdf` changes while the update fetches.
    let ssh = sandbox.root().join("ssh");
    write(
        &ssh,
        "#!/bin/sh\nfor last; do :; done\n[ -z \"$EDIT\" ] || echo edit > \"$EDIT\"\nexec sh -c \"$last\"\n",
    );
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let url = format!("ssh://fake{}", up.path().display());
    let skiman = |args: &[&str]| {
        sandbox
            .skiman()
            .args(args)
            .env("GIT_SSH_COMMAND", &ssh)
            .env("GIT_SSH_VARIANT", "simple")
    };
    skiman(&["add", &url, "--yes"]).assert().code(0);
    let repo = sandbox.skill_repo();
    write(
        repo.join("skills/pdf/forms.md"),
        lines(&[(1, "local edit")]),
    );
    up.write("forms.md", lines(&[(20, "upstream fix")]));
    up.commit("v2");
    skiman(&["update", "pdf"])
        .env("EDIT", repo.join("upstream/pdf/forms.md"))
        .assert()
        .code(0);
    assert_eq!(
        read(repo.join("skills/pdf/forms.md")),
        lines(&[(1, "local edit"), (20, "upstream fix")])
    );
    assert!(read(repo.join("skiman.lock")).contains(&up.head()));
    sandbox.skiman().arg("check").assert().code(0);
}

/// update reads a skill's records without the repo lock. Read while another
/// command was writing them (here: the lock entry not there yet), they
/// refuse the skill; read again under the lock they are whole, so the
/// update goes on with them instead of sending the user to repair a skill
/// repo that is fine.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn records_read_while_another_command_writes_them_are_read_again_under_the_lock() {
    let sandbox = Sandbox::new();
    let up = pdf_with_an_update(&sandbox);
    let lock = sandbox.skill_repo().join("skiman.lock");
    let aside = sandbox.root().join("skiman.lock");
    std::fs::rename(&lock, &aside).unwrap();

    let assert = update_with_hook(
        &sandbox,
        &["pdf"],
        "pdf:read-again",
        &format!("mv '{}' '{}'", aside.display(), lock.display()),
    )
    .code(0);
    assert_eq!(stderr_of(&assert), "");
    let stdout = stdout_of(&assert);
    assert!(
        stdout.starts_with(&format!("skill pdf: updated to {}", up.head())),
        "{stdout}"
    );
    assert!(read(&lock).contains(&up.head()));
    sandbox.skiman().arg("check").assert().code(0);
}

const NEWER: &str = "edit made during the update\n";

/// An edit to `skills/<name>` between the repo lock and the swap stops the
/// skill before it is touched: exit 4, nothing written, the newer edit kept.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_edit_while_the_update_writes_stops_it_before_the_skill_is_replaced() {
    let sandbox = Sandbox::new();
    pdf_with_an_update(&sandbox);
    let repo = sandbox.skill_repo();
    let forms = repo.join("skills/pdf/forms.md");
    let mut expected = tree(&repo);

    update_with_hook(
        &sandbox,
        &["pdf"],
        "update:write:skills/pdf",
        &format!("printf '{}' > '{}'", NEWER.trim_end(), forms.display()),
    )
    .code(4)
    .stdout_eq("")
    .stderr_eq(format!(
        "error: skill pdf: {} changed while skiman was writing; nothing was put in its place\n",
        repo.join("skills/pdf").display()
    ));

    expected.insert(
        "skills/pdf/forms.md".into(),
        format!("file 644 {}", NEWER.trim_end()),
    );
    assert_eq!(tree(&repo), expected);
    let saved: Vec<_> = backup_dirs(&sandbox).iter().flat_map(tree).collect();
    assert!(saved.iter().all(|(_, v)| v == "dir"), "{saved:?}");
}

fn chmod(path: impl AsRef<Path>, mode: u32) {
    std::fs::set_permissions(path.as_ref(), std::fs::Permissions::from_mode(mode)).unwrap();
}

fn stdout_of(assert: &snapbox::cmd::OutputAssert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

fn stderr_of(assert: &snapbox::cmd::OutputAssert) -> String {
    String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
}

/// A lock entry that cannot be written stops the update (exit 4) after
/// what was written before it, which it lists; once the cause is gone, the
/// same command finishes it: the skill repo ends as an update that never
/// stopped leaves it. With `--ref`, `skiman.toml` is written before the
/// lock entry; with the same snapshot, only the records move.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_failed_lock_write_stops_the_update_and_running_it_again_finishes_it() {
    // How the sandbox is made, the command, and how its last line before
    // the lock entry starts (none: nothing comes before it).
    type Setup = fn(&Sandbox);
    let setups: [(Setup, &[&str], &str); 3] = [
        (
            |s| {
                pdf_with_an_update(s);
            },
            &["update", "pdf"],
            "moved",
        ),
        (
            |s| {
                pdf_with_an_update(s).branch("stable");
            },
            &["update", "pdf", "--ref", "stable"],
            "changed",
        ),
        (
            |s| {
                init(s);
                installed(s, "pdf").commit("an empty commit");
            },
            &["update", "pdf"],
            "",
        ),
    ];
    let state = |s: &Sandbox| {
        let mut state = final_state(s);
        state.retain(|k, _| !k.starts_with("repo/.skiman"));
        state
    };
    for (setup, args, before_lock) in setups {
        let uninterrupted = Sandbox::new();
        setup(&uninterrupted);
        uninterrupted.skiman().args(args).assert().code(0);

        let sandbox = Sandbox::new();
        setup(&sandbox);
        let repo = sandbox.skill_repo();
        let assert = sandbox
            .skiman()
            .args(args)
            .env("SKIMAN_TEST_HOOK", "update:write:skiman.lock")
            .env(
                "SKIMAN_TEST_HOOK_RUN",
                format!("chmod 555 '{}'", repo.display()),
            )
            .assert();
        chmod(&repo, 0o755);
        let assert = assert.code(4);
        let (stdout, stderr) = (stdout_of(&assert), stderr_of(&assert));
        let r = repo.display();
        assert!(
            stderr.starts_with(&format!(
                "error: skill pdf: cannot write {r}/skiman.lock: Permission denied (os error 13)"
            )),
            "{stderr}"
        );
        if before_lock.is_empty() {
            assert_eq!(stdout, "");
        } else {
            let last = stdout.lines().last().unwrap_or_default();
            assert!(last.starts_with(before_lock), "{stdout}");
            assert!(
                stderr.ends_with("run the same command again to finish it\n"),
                "{stderr}"
            );
        }

        sandbox.skiman().args(args).assert().code(0);
        assert_eq!(state(&sandbox), state(&uninterrupted), "{args:?}");
    }
}

/// A piece of the wrong kind (`upstream/<name>` a file) stops the update
/// before anything is written: exit 4, the skill and its records as they
/// were.
#[test]
fn a_piece_of_the_wrong_kind_stops_the_update_before_anything_is_written() {
    let sandbox = Sandbox::new();
    pdf_with_an_update(&sandbox);
    let repo = sandbox.skill_repo();
    std::fs::remove_dir_all(repo.join("upstream/pdf")).unwrap();
    write(repo.join("upstream/pdf"), "not a folder\n");
    let before = [tree(repo.join("skills")), tree(repo.join("upstream"))];
    let lock = read(repo.join("skiman.lock"));

    let update = sandbox.skiman().args(["update", "pdf"]).assert().code(4);

    let stderr = stderr_of(&update);
    assert!(
        stderr.contains(
            "is in the way: it is not of the kind skiman writes there; nothing was changed"
        ),
        "{stderr}"
    );
    assert_eq!(stdout_of(&update), "");
    assert_eq!(
        [tree(repo.join("skills")), tree(repo.join("upstream"))],
        before
    );
    assert_eq!(read(repo.join("skiman.lock")), lock);
}

/// The new `skills/<name>` is in place, but the old one cannot go to the
/// backup: exit 4, the old one stays in `.skiman/tmp/` as a tmp leftover,
/// and the same command finishes the update, moving it to a backup first.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_failed_move_of_the_old_skill_to_the_backup_keeps_it_and_running_again_finishes_the_update() {
    let sandbox = Sandbox::new();
    let up = pdf_with_an_update(&sandbox);
    let repo = sandbox.skill_repo();
    let skill_before = tree(repo.join("skills/pdf"));

    let assert = update_with_hook(
        &sandbox,
        &["pdf"],
        "update:write:skills/pdf",
        "chmod 555 \"$XDG_STATE_HOME\"/skiman/backups/*",
    )
    .code(4);
    assert!(backup_dirs(&sandbox).is_empty(), "nothing went in");
    let (stdout, stderr) = (stdout_of(&assert), stderr_of(&assert));
    let skill = repo.join("skills/pdf");
    assert_eq!(stdout, format!("created {}\n", skill.display()));
    assert!(
        stderr.contains(&format!(
            "the new {} is in place, but the old one cannot be moved to the backup from ",
            skill.display()
        )),
        "{stderr}"
    );
    assert!(
        stderr.ends_with("run the same command again to finish it\n"),
        "{stderr}"
    );
    assert_eq!(
        read(repo.join("skills/pdf/reference.md")),
        lines(&[(20, "upstream fix")])
    );

    let again = sandbox.skiman().args(["update", "pdf"]).assert().code(0);
    let stdout = stdout_of(&again);
    let moved = stdout
        .lines()
        .find_map(|l| l.strip_prefix("moved tmp leftovers to "))
        .unwrap_or_else(|| panic!("{stdout}"));
    let old = std::fs::read_dir(moved)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(tree(old.join("old/skills/pdf")), skill_before);
    assert!(read(repo.join("skiman.lock")).contains(&up.head()));
    sandbox.skiman().arg("check").assert().code(0);
}

/// The swap went through, but the folder it happened in cannot be flushed
/// (one skiman can write in but not read, 0333): exit 4, saying so; the new
/// skill is in place, the old one whole in the backup, and the same command
/// run again finishes the update.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_folder_that_cannot_be_flushed_after_the_swap_keeps_the_old_skill() {
    let sandbox = Sandbox::new();
    let up = pdf_with_an_update(&sandbox);
    let repo = sandbox.skill_repo();
    let skill_before = tree(repo.join("skills/pdf"));
    let skills = repo.join("skills");

    let assert = update_with_hook(
        &sandbox,
        &["pdf"],
        "update:write:skills/pdf",
        &format!("chmod 333 '{}'", skills.display()),
    );
    chmod(&skills, 0o755);
    let assert = assert.code(4);
    let stderr = stderr_of(&assert);
    assert!(
        stderr.contains(&format!(
            "the new {}/pdf is in place and the old one in the backup, but its folder cannot be flushed: ",
            skills.display()
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains("run the same command again to finish it\n"),
        "{stderr}"
    );
    let backup = backup_dirs(&sandbox).pop().unwrap();
    assert_eq!(tree(backup.join("repo/skills/pdf")), skill_before);
    assert_eq!(
        read(repo.join("skills/pdf/reference.md")),
        lines(&[(20, "upstream fix")])
    );

    sandbox.skiman().args(["update", "pdf"]).assert().code(0);
    assert!(read(repo.join("skiman.lock")).contains(&up.head()));
    sandbox.skiman().arg("check").assert().code(0);
}

/// A backup folder that cannot be made stops the skill before anything is
/// written.
#[test]
fn an_unwritable_backups_directory_stops_the_skill_before_anything_is_swapped() {
    let sandbox = Sandbox::new();
    pdf_with_an_update(&sandbox);
    let before = tree(sandbox.skill_repo());
    std::fs::create_dir_all(backups(&sandbox)).unwrap();
    chmod(backups(&sandbox), 0o555);

    let assert = sandbox.skiman().args(["update", "pdf"]).assert();
    chmod(backups(&sandbox), 0o755);
    assert.code(4).stdout_eq("").stderr_eq(format!(
        "error: skill pdf: cannot create a backup folder in {}: Permission denied (os error 13); nothing was changed\n",
        backups(&sandbox).display()
    ));

    assert_eq!(tree(sandbox.skill_repo()), before);
    assert!(backup_dirs(&sandbox).is_empty());
}

/// Killed right before each of its pieces, an update finishes when run again,
/// ending as one that was never killed: until the lock entry is written it
/// recognises the merge in place from what the killed run swapped out, after
/// it the base comes from the new entry.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_update_killed_at_any_step_finishes_when_run_again() {
    assert_each_kill_finishes(
        |sandbox| {
            pdf_with_an_update(sandbox);
            vec!["update".into(), "pdf".into()]
        },
        &[
            "update:write:skills/pdf",
            "update:write:skiman.lock",
            "update:write:upstream/pdf",
        ],
    );
    assert_each_kill_finishes(
        |sandbox| {
            pdf_with_an_update(sandbox).branch("stable");
            ["update", "pdf", "--ref", "stable"]
                .map(String::from)
                .to_vec()
        },
        &["update:write:skiman.toml", "update:write:skiman.lock"],
    );
}

/// `pdf` installed with a local change next to repeated lines, and a new
/// upstream version that adds one more of them; returns the update's
/// arguments.
fn pdf_with_repeated_lines(sandbox: &Sandbox) -> Vec<String> {
    pdf_with_repeated_lines_in(sandbox, None)
}

/// [`pdf_with_repeated_lines`], with `pdf` in the group folder `group`.
fn pdf_with_repeated_lines_in(sandbox: &Sandbox, group: Option<&str>) -> Vec<String> {
    let text = |lines: &[&str]| lines.iter().map(|l| format!("{l}\n")).collect::<String>();
    init(sandbox);
    let up = sandbox.upstream("acme/pdf");
    up.write(
        "SKILL.md",
        "---\nname: pdf\ndescription: The pdf skill\n---\n",
    )
    .write("reference.md", text(&["c", "}", "c", "", "}", "}", "}"]));
    up.commit("v1");
    let mut args = vec!["add", "acme/pdf", "--yes"];
    args.extend(group.iter().flat_map(|group| ["--group", group]));
    sandbox.skiman().args(args).assert().code(0);
    let skill = match group {
        Some(group) => format!("skills/{group}/pdf"),
        None => "skills/pdf".to_owned(),
    };
    let local = text(&["c", "}", "- item", "}", "}"]);
    write(sandbox.skill_repo().join(skill).join("reference.md"), local);
    up.write(
        "reference.md",
        text(&["c", "}", "c", "", "}", "}", "}", "}"]),
    );
    up.commit("v2");
    vec!["update".into(), "pdf".into()]
}

/// Stopped after it wrote `skills/<name>`, before the lock, an update run
/// again keeps the merge in place instead of merging it again, which here
/// would apply upstream's change twice (an added `}` among repeated lines,
/// next to the local change). It finds what the stopped run swapped out in
/// that run's backup (killed) or in `.skiman/tmp/` (its move to the backup
/// failed). The kill at `upstream/<name>` also covers the step after the
/// lock.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_update_stopped_before_the_lock_does_not_merge_again() {
    assert_each_kill_finishes(
        pdf_with_repeated_lines,
        &["update:write:skiman.lock", "update:write:upstream/pdf"],
    );

    let (never, stopped) = (Sandbox::new(), Sandbox::new());
    let args = pdf_with_repeated_lines(&never);
    never.skiman().args(&args).assert().code(0);
    pdf_with_repeated_lines(&stopped);
    let chmod = "chmod 555 \"$XDG_STATE_HOME\"/skiman/backups/*";
    update_with_hook(&stopped, &["pdf"], "update:write:skills/pdf", chmod).code(4);
    stopped.skiman().args(&args).assert().code(0);
    let never_stopped = final_state(&never);
    assert_eq!(final_state(&stopped), never_stopped);

    // Without a swapping rename the old folder waits at `<name>.skiman-prev`;
    // earlier versions left it at `old/prev/`. The run again finds either.
    for layout in ["skills/pdf.skiman-prev", "prev/skills/pdf"] {
        let stopped = Sandbox::new();
        pdf_with_repeated_lines(&stopped);
        update_with_hook(&stopped, &["pdf"], "update:write:skills/pdf", chmod).code(4);
        let tmp = stopped.skill_repo().join(".skiman/tmp");
        let leftovers: Vec<_> = std::fs::read_dir(&tmp)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let [leftover] = &leftovers[..] else {
            panic!("{leftovers:?}")
        };
        let to = leftover.join("old").join(layout);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::rename(leftover.join("old/skills/pdf"), &to).unwrap();
        stopped.skiman().args(&args).assert().code(0);
        assert_eq!(final_state(&stopped), never_stopped, "{layout}");
    }

    // The next write command moves what the update swapped out to its own
    // backup; the update run again finds it there.
    let (never, moved) = (Sandbox::new(), Sandbox::new());
    let new = ["new", "other", "-d", "Use when testing."];
    pdf_with_repeated_lines(&never);
    never.skiman().args(&args).assert().code(0);
    never.skiman().args(new).assert().code(0);
    pdf_with_repeated_lines(&moved);
    update_with_hook(&moved, &["pdf"], "update:write:skills/pdf", chmod).code(4);
    moved.skiman().args(new).assert().code(0);
    moved.skiman().args(&args).assert().code(0);
    assert_eq!(final_state(&moved), final_state(&never));
}

/// The same for a skill in a nested group folder: killed at each write, or stopped
/// with what it swapped out still in `.skiman/tmp/` (`old/skills/<group>/<name>`),
/// the update run again finds the old folder and does not merge again.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_update_of_a_grouped_skill_stopped_before_the_lock_does_not_merge_again() {
    let grouped = |sandbox: &Sandbox| pdf_with_repeated_lines_in(sandbox, Some("documents/forms"));
    assert_each_kill_finishes(
        grouped,
        &[
            "update:write:skills/documents/forms/pdf",
            "update:write:skiman.lock",
            "update:write:upstream/pdf",
        ],
    );

    let (never, stopped) = (Sandbox::new(), Sandbox::new());
    let args = grouped(&never);
    never.skiman().args(&args).assert().code(0);
    grouped(&stopped);
    let chmod = "chmod 555 \"$XDG_STATE_HOME\"/skiman/backups/*";
    let at = "update:write:skills/documents/forms/pdf";
    update_with_hook(&stopped, &["pdf"], at, chmod).code(4);
    stopped.skiman().args(&args).assert().code(0);
    assert_eq!(final_state(&stopped), final_state(&never));
}

/// What a replace stopped between its two renames left for a skill folder in
/// one group is never put back once the skill sits in another: `mv` moves
/// every tmp leftover to the backup before it moves the folder, and update
/// looks for a set-aside skill only when the skill is in no folder at all.
#[test]
fn a_leftover_of_another_group_is_never_put_back() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    installed(&sandbox, "pdf");
    let repo = sandbox.skill_repo();
    let stopped = |group: &str| {
        let old = repo.join(format!(".skiman/tmp/stopped-{group}/old/skills/{group}"));
        write(old.join("pdf/SKILL.md"), "the new one\n");
        write(old.join("pdf.skiman-prev/SKILL.md"), "the old one\n");
    };
    let mv = |group: &str| {
        let args = ["mv", "pdf", "--group", group];
        sandbox.skiman().args(args).assert().code(0);
    };
    mv("a");
    let pdf = tree(repo.join("skills/a/pdf"));

    // A leftover for `skills/b/pdf`: the move to `b` puts pdf's own folder there.
    stopped("b");
    mv("b");
    assert_eq!(tree(repo.join("skills/b/pdf")), pdf);
    assert!(!repo.join(".skiman/tmp/stopped-b").exists());

    // A leftover for `skills/a/pdf`, with `a` still a group folder: an update
    // that cannot read pdf's records refuses and puts nothing in `a`.
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes", "--group", "a"])
        .assert()
        .code(0);
    stopped("a");
    let lock = repo.join("skiman.lock");
    std::fs::write(&lock, read(&lock).replace("[skills.pdf]", "[skills.gone]")).unwrap();
    sandbox.skiman().args(["update", "pdf"]).assert().code(1);
    assert!(!repo.join("skills/a/pdf").exists());
    assert_eq!(tree(repo.join("skills/b/pdf")), pdf);
    assert!(repo.join(".skiman/tmp/stopped-a/old/skills/a/pdf").is_dir());
}
