//! `skiman sync [--hook]`: link distribution, `skiman-manual` and the canon lock.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use snapbox::str;

use crate::support::{
    Sandbox, Tool, ToolSetup, assert_each_kill_finishes, assert_symlink, exists, init, killed_at,
    logging_git, moved_note, tree, write,
};

fn write_skill(dir: &std::path::Path, name: &str) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    std::fs::write(
        dir.join(name).join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\n---\n"),
    )
    .unwrap();
}

#[test]
fn sync_links_a_skill_folder_created_by_hand() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.canon().join("skills"), "notes");

    sandbox
        .skiman_confined()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
linked [..]/home/.claude/skills/notes
linked [..]/home/.agents/skills/notes
linked [..]/home/.gemini/config/skills/notes

"#]])
        .stderr_eq("");

    let target = sandbox.canon().join("skills/notes");
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("notes"), &target, &target);
    }
    // What the git hooks run, confined too (macOS): it links a skill made since.
    write_skill(&sandbox.canon().join("skills"), "memo");
    sandbox
        .skiman_confined()
        .args(["sync", "--hook"])
        .assert()
        .code(0);
    let target = sandbox.canon().join("skills/memo");
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("memo"), &target, &target);
    }
}

#[test]
fn sync_links_only_distributable_skill_folders() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skills = sandbox.canon().join("skills");
    write_skill(&skills, "Bad_Name");
    write_skill(&skills, "synced");
    std::fs::create_dir(skills.join("empty")).unwrap();
    std::fs::write(skills.join("loose-file"), "x").unwrap();

    sandbox.skiman().arg("sync").assert().code(0).stdout_eq("");

    for tool in Tool::ALL {
        let names: Vec<_> = tree(sandbox.tool_dir(tool)).into_keys().collect();
        assert_eq!(names, [std::path::PathBuf::from("skiman-manual")]);
    }
}

#[test]
fn skiman_manual_is_linked_everywhere_holds_the_repo_path_and_is_not_rewritten() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.canon();
    let materialised = repo.join(".skiman/skiman-manual");

    for tool in Tool::ALL {
        assert_symlink(
            sandbox.tool_dir(tool).join("skiman-manual"),
            &materialised,
            &materialised,
        );
    }
    let skill_md = materialised.join("SKILL.md");
    let content = std::fs::read_to_string(&skill_md).unwrap();
    assert!(content.starts_with("---\nname: skiman-manual\ndescription: "));
    assert!(
        content.contains(&format!("The canon is `{}`.", repo.display())),
        "{content}"
    );
    let inode = std::fs::metadata(&skill_md).unwrap().ino();

    sandbox.skiman().arg("sync").assert().code(0).stdout_eq("");

    assert_eq!(std::fs::metadata(&skill_md).unwrap().ino(), inode);
    assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), content);
}

#[test]
fn skiman_manual_is_rewritten_when_its_content_differs() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skill_md = sandbox.canon().join(".skiman/skiman-manual/SKILL.md");
    let content = std::fs::read_to_string(&skill_md).unwrap();
    std::fs::write(&skill_md, "edited").unwrap();

    sandbox.skiman().arg("sync").assert().code(0);

    assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), content);
}

#[test]
fn sync_hook_does_not_write_skiman_manual_through_a_linked_folder_and_names_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let materialised = sandbox.canon().join(".skiman/skiman-manual");
    std::fs::remove_dir_all(&materialised).unwrap();
    let foreign = sandbox.root().join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("SKILL.md"), "user original").unwrap();
    std::os::unix::fs::symlink(&foreign, &materialised).unwrap();

    let assert = sandbox.skiman().args(["sync", "--hook"]).assert();

    assert_eq!(
        std::fs::read_to_string(foreign.join("SKILL.md")).unwrap(),
        "user original",
        "sync wrote through the link"
    );
    assert_eq!(std::fs::read_link(&materialised).unwrap(), foreign);
    assert.code(0).stdout_eq("").stderr_eq(str![[r#"
error: cannot write skiman-manual: [..]/canon/.skiman/skiman-manual is not a real folder (a link or a file is in the way)

"#]]);
}

/// The manual as a release before the rename left it: materialised at
/// `.skiman/managing-skills` and linked under that name in every tool
/// directory. Returns the old folder.
fn old_manual(sandbox: &Sandbox) -> std::path::PathBuf {
    init(sandbox);
    let local = sandbox.canon().join(".skiman");
    let old = local.join("managing-skills");
    std::fs::rename(local.join("skiman-manual"), &old).unwrap();
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        std::fs::remove_file(dir.join("skiman-manual")).unwrap();
        std::os::unix::fs::symlink(&old, dir.join("managing-skills")).unwrap();
    }
    old
}

/// `skiman doctor` reports no tool-directory entry under either name of
/// the manual: neither is foreign nor occupied.
fn assert_doctor_quiet_about_the_manual(sandbox: &Sandbox) {
    let output = sandbox.skiman().arg("doctor").assert().code(0);
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    for tool in Tool::ALL {
        for name in ["managing-skills", "skiman-manual"] {
            let entry = sandbox.tool_dir(tool).join(name).display().to_string();
            assert!(!stdout.contains(&entry), "{stdout}");
        }
    }
}

#[test]
fn the_first_sync_after_the_rename_moves_the_manual_to_skiman_manual() {
    let sandbox = Sandbox::new();
    let old = old_manual(&sandbox);

    sandbox.skiman().arg("sync").assert().code(0).stderr_eq("");

    let manual = sandbox.canon().join(".skiman/skiman-manual");
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert_symlink(dir.join("skiman-manual"), &manual, &manual);
        assert!(!exists(dir.join("managing-skills")), "{}", tool.tool_dir());
    }
    assert!(!exists(&old), "the old manual folder is left");
    assert_doctor_quiet_about_the_manual(&sandbox);
}

/// The name `managing-skills` is free again: a linked skill with that name
/// takes the place of the old manual's link in the same sync.
#[test]
fn the_first_sync_after_the_rename_gives_managing_skills_to_a_linked_skill() {
    let sandbox = Sandbox::new();
    let old = old_manual(&sandbox);
    let company = sandbox.home().join("work/company");
    write_skill(&company, "managing-skills");
    let manifest = sandbox.canon().join("skiman.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        format!("{text}\n[linked.company]\npath = \"~/work/company\"\n"),
    )
    .unwrap();

    sandbox.skiman().arg("sync").assert().code(0).stderr_eq("");

    let linked = sandbox.canon().join("linked/company/managing-skills");
    let real = company.join("managing-skills");
    for tool in Tool::ALL {
        assert_symlink(
            sandbox.tool_dir(tool).join("managing-skills"),
            &linked,
            &real,
        );
    }
    assert!(!exists(&old), "the old manual folder is left");
    assert_doctor_quiet_about_the_manual(&sandbox);
}

/// The doctor line for a removal of `path` the next sync makes.
fn removes(path: &Path) -> String {
    format!("{}: the next `skiman sync` removes it", path.display())
}

/// doctor foresees the first sync after the rename: it names the old
/// manual's folder and links as removed and the new links as made, and
/// leaves the old folder there; the sync then does exactly that.
#[test]
fn doctor_foresees_the_first_sync_after_the_rename() {
    let sandbox = Sandbox::new();
    let old = old_manual(&sandbox);

    let out = crate::doctor::assert_doctor_foresees_sync(&sandbox);

    assert!(out.lines().any(|l| l == removes(&old)), "{out}");
    for tool in Tool::ALL {
        let link = sandbox.tool_dir(tool).join("managing-skills");
        assert!(out.lines().any(|l| l == removes(&link)), "{out}");
    }
    assert!(!exists(&old), "the old manual folder is left");
}

/// The new manual's folder is a link: the manual cannot be written, so the
/// old one stays too, and doctor says what sync says.
#[test]
fn the_old_manual_stays_while_the_new_one_cannot_be_written() {
    let sandbox = Sandbox::new();
    let old = old_manual(&sandbox);
    let manual = sandbox.canon().join(".skiman/skiman-manual");
    let elsewhere = sandbox.root().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &manual).unwrap();

    let out = crate::doctor::assert_doctor_foresees_sync(&sandbox);

    assert!(
        out.contains(&format!("cannot write skiman-manual: {}", manual.display())),
        "{out}"
    );
    assert!(!out.lines().any(|l| l == removes(&old)), "{out}");
    assert!(exists(old.join("SKILL.md")), "the old manual folder went");
}

/// A linked skill named `managing-skills` takes the old manual's name in
/// the first sync after the rename; doctor foresees each retarget.
#[test]
fn doctor_foresees_a_linked_skill_taking_the_old_manuals_name() {
    let sandbox = Sandbox::new();
    let old = old_manual(&sandbox);
    write_skill(&sandbox.home().join("work/company"), "managing-skills");
    let manifest = sandbox.canon().join("skiman.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let text = format!("{text}\n[linked.company]\npath = \"~/work/company\"\n");
    std::fs::write(&manifest, text).unwrap();

    let out = crate::doctor::assert_doctor_foresees_sync(&sandbox);

    assert!(out.lines().any(|l| l == removes(&old)), "{out}");
    for tool in Tool::ALL {
        let link = sandbox.tool_dir(tool).join("managing-skills");
        let line = format!("{}: the next `skiman sync` retargets it", link.display());
        assert!(out.lines().any(|l| l == line), "{out}");
    }
}

/// The old manual's place holds a link: sync removes the link, not what
/// it leads to.
#[test]
fn the_old_manual_as_a_link_goes_without_what_it_leads_to() {
    let sandbox = Sandbox::new();
    let old = old_manual(&sandbox);
    let kept = sandbox.root().join("kept");
    std::fs::rename(&old, &kept).unwrap();
    std::os::unix::fs::symlink(&kept, &old).unwrap();

    let out = crate::doctor::assert_doctor_foresees_sync(&sandbox);

    assert!(out.lines().any(|l| l == removes(&old)), "{out}");
    assert!(std::fs::symlink_metadata(&old).is_err());
    assert!(exists(kept.join("SKILL.md")), "what the link led to went");
}

#[test]
fn sync_skips_a_tool_whose_presence_directory_is_missing_and_creates_a_missing_tool_directory() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Codex, ToolSetup::PresenceOnly)
        .build();
    init(&sandbox);
    std::fs::remove_dir_all(sandbox.presence_dir(Tool::Antigravity)).unwrap();
    write_skill(&sandbox.canon().join("skills"), "notes");

    sandbox.skiman().arg("sync").assert().code(0);

    let target = sandbox.canon().join("skills/notes");
    assert_symlink(
        sandbox.tool_dir(Tool::Codex).join("notes"),
        &target,
        &target,
    );
    assert!(!sandbox.presence_dir(Tool::Antigravity).exists());
}

#[test]
fn sync_hook_with_nothing_to_change_prints_nothing_and_exits_0() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.canon().join("skills"), "notes");
    sandbox.skiman().arg("sync").assert().code(0);
    let before: Vec<_> = Tool::ALL
        .iter()
        .map(|t| tree(sandbox.tool_dir(*t)))
        .collect();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");

    let after: Vec<_> = Tool::ALL
        .iter()
        .map(|t| tree(sandbox.tool_dir(*t)))
        .collect();
    assert_eq!(after, before);
}

#[test]
fn sync_hook_links_new_skills_without_printing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.canon().join("skills"), "notes");

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");

    let target = sandbox.canon().join("skills/notes");
    assert_symlink(
        sandbox.tool_dir(Tool::ClaudeCode).join("notes"),
        &target,
        &target,
    );
}

/// `sync`, with the test hook writing a regular file to `path` after sync
/// found `path` empty and right before it creates its link there.
fn sync_while_a_file_appears_at(sandbox: &Sandbox, path: &Path) -> snapbox::cmd::OutputAssert {
    sandbox
        .skiman()
        .arg("sync")
        .env("SKIMAN_TEST_HOOK", format!("sync:link:{}", path.display()))
        .env(
            "SKIMAN_TEST_HOOK_RUN",
            format!("printf mine > '{}'", path.display()),
        )
        .assert()
}

/// `path` is still the file the test hook wrote.
fn assert_kept(path: &Path) {
    let meta = std::fs::symlink_metadata(path).unwrap();
    assert!(meta.is_file(), "{} was replaced by a link", path.display());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "mine");
}

#[test]
fn a_file_that_appears_where_sync_is_about_to_link_a_skill_is_kept_and_reported_as_occupied() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.canon().join("skills"), "notes");
    let path = sandbox.tool_dir(Tool::ClaudeCode).join("notes");

    let assert = sync_while_a_file_appears_at(&sandbox, &path);

    assert_kept(&path);
    assert
        .code(3)
        .stdout_eq(str![[r#"
linked [..]/home/.agents/skills/notes
linked [..]/home/.gemini/config/skills/notes

"#]])
        .stderr_eq(str![[r#"
warning: [..]/home/.claude/skills/notes is occupied by an entry skiman does not own, so skill notes is not linked there; move it away, then run `skiman sync`

"#]]);
}

#[test]
fn a_file_that_appears_where_sync_is_about_to_link_the_shared_body_is_kept_and_reported_as_refused()
{
    let sandbox = Sandbox::new();
    init(&sandbox);
    let path = sandbox.instruction_file(Tool::ClaudeCode);
    std::fs::remove_file(&path).unwrap();

    let assert = sync_while_a_file_appears_at(&sandbox, &path);

    assert_kept(&path);
    assert.code(3).stdout_eq("").stderr_eq(str![[r#"
warning: instruction file [..]/home/.claude/CLAUDE.md is not owned by skiman, so the shared body is not linked there; move it away, then run `skiman sync`

"#]]);
}

/// A canon whose next sync removes two of skiman's own links: the
/// skill `notes` is gone, and Antigravity is no longer enabled.
fn two_links_to_remove(sandbox: &Sandbox) {
    init(sandbox);
    write_skill(&sandbox.canon().join("skills"), "notes");
    sandbox.skiman().arg("sync").assert().code(0);
    std::fs::remove_dir_all(sandbox.canon().join("skills/notes")).unwrap();
    std::fs::write(
        sandbox.canon().join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\", \"codex\"]\n",
    )
    .unwrap();
}

/// A regular file of the user's that takes the place of skiman's own link
/// at `path` right before sync removes it is kept, never removed.
#[test]
fn a_file_that_replaces_a_link_sync_is_about_to_remove_is_kept() {
    let paths = [
        |sandbox: &Sandbox| sandbox.tool_dir(Tool::ClaudeCode).join("notes"),
        |sandbox: &Sandbox| sandbox.instruction_file(Tool::Antigravity),
    ];
    for path in paths {
        let sandbox = Sandbox::new();
        two_links_to_remove(&sandbox);
        let path = path(&sandbox);
        let swap_in = format!("rm '{0}' && printf mine > '{0}'", path.display());
        let synced = sandbox
            .skiman()
            .arg("sync")
            .env("SKIMAN_TEST_HOOK", format!("link:take:{}", path.display()))
            .env("SKIMAN_TEST_HOOK_RUN", swap_in)
            .output()
            .unwrap();
        assert_eq!(synced.status.code(), Some(0), "{synced:?}");
        assert_kept(&path);
        let siblings = std::fs::read_dir(path.parent().unwrap()).unwrap();
        let names: Vec<_> = siblings.map(|e| e.unwrap().file_name()).collect();
        assert!(
            !names
                .iter()
                .any(|n| n.to_string_lossy().contains("skiman-tmp")),
            "{names:?}"
        );
    }
}

/// Killed with a link moved aside to its temporary name, sync leaves it
/// there; the next sync removes it as skiman's own.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_sync_killed_while_it_removes_a_link_finishes_when_run_again() {
    assert_each_kill_finishes(
        |sandbox| {
            two_links_to_remove(sandbox);
            vec!["sync".to_owned()]
        },
        &[
            "link:aside:<root>/home/.claude/skills/notes",
            "link:aside:<root>/home/.gemini/GEMINI.md",
        ],
    );
}

/// An instruction link left under its temporary name stays through
/// `sync --hook`, which reads no folder around an instruction location, and
/// goes with the next `skiman sync`.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn an_instruction_link_left_aside_goes_with_the_next_sync_not_the_hook() {
    let sandbox = Sandbox::new();
    two_links_to_remove(&sandbox);
    let gemini = sandbox.instruction_file(Tool::Antigravity);
    let hook = format!("link:aside:{}", gemini.display());
    killed_at(&sandbox, &["sync".to_owned()], &hook);
    let left = || {
        let dir = std::fs::read_dir(gemini.parent().unwrap()).unwrap();
        let names = dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned());
        names.filter(|n| n.contains(".skiman-tmp-")).count()
    };
    assert_eq!(left(), 1);
    sandbox.skiman().args(["sync", "--hook"]).assert().code(0);
    assert_eq!(left(), 1);
    sandbox.skiman().arg("sync").assert().code(0);
    assert_eq!(left(), 0);
}

/// A sync killed again while it took a left link aside leaves it under a
/// temporary name of a temporary name: the next `skiman sync` removes it as
/// skiman's own, in a tool directory and beside an instruction file.
#[test]
fn a_link_left_aside_twice_goes_with_the_next_sync() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.canon().join("skills"), "notes");
    sandbox.skiman().arg("sync").assert().code(0);
    let skill = sandbox.tool_dir(Tool::ClaudeCode).join("notes");
    let claude_md = sandbox.instruction_file(Tool::ClaudeCode);
    let mut left = Vec::new();
    for (link, name) in [(&skill, "notes"), (&claude_md, "CLAUDE.md")] {
        let twice = link.with_file_name(format!("..{name}.skiman-tmp-7.skiman-tmp-8-1"));
        std::os::unix::fs::symlink(std::fs::read_link(link).unwrap(), &twice).unwrap();
        left.push(twice);
    }

    sandbox.skiman().arg("sync").assert().code(0);

    for twice in &left {
        assert!(!twice.is_symlink(), "{} is still there", twice.display());
    }
    assert!(skill.is_symlink() && claude_md.is_symlink());
}

/// A link named like one sync took aside is skiman's only while it points
/// where skiman's own link of that name does: one that points anywhere
/// else, another skill included, is the user's, and sync leaves it.
#[test]
fn a_link_under_a_temporary_name_that_points_elsewhere_is_not_skimans() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skills = sandbox.canon().join("skills");
    write_skill(&skills, "notes");
    write_skill(&skills, "other");
    sandbox.skiman().arg("sync").assert().code(0);
    let theirs = sandbox.root().join("theirs");
    std::fs::create_dir(&theirs).unwrap();
    let tool_dir = sandbox.tool_dir(Tool::ClaudeCode);
    let claude_md = sandbox.instruction_file(Tool::ClaudeCode);
    let links = [
        (tool_dir.join(".notes.skiman-tmp-7"), theirs.clone()),
        (tool_dir.join(".notes.skiman-tmp-8"), skills.join("other")),
        (claude_md.with_file_name(".CLAUDE.md.skiman-tmp-7"), theirs),
    ];
    for (link, target) in &links {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    sandbox.skiman().arg("sync").assert().code(0);

    for (link, target) in &links {
        assert_eq!(std::fs::read_link(link).ok().as_ref(), Some(target));
    }
}

/// A link into a group folder is skiman's under its own name and under a
/// temporary name, whatever the group: a skill moved to another group by
/// hand gets its links retargeted by the next sync, and the links taken
/// aside to its old folder go. A temporary name that points at another
/// skill in the same group stays the user's.
#[test]
fn links_into_group_folders_are_skimans_under_their_name_or_a_temporary_one() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skills = sandbox.canon().join("skills");
    write_skill(&skills.join("docs"), "notes");
    write_skill(&skills.join("docs"), "other");
    sandbox.skiman().arg("sync").assert().code(0);
    let old = skills.join("docs/notes");
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("notes"), &old, &old);
    }
    let tool_dir = sandbox.tool_dir(Tool::ClaudeCode);
    let aside = [
        tool_dir.join(".notes.skiman-tmp-7"),
        tool_dir.join("..notes.skiman-tmp-7.skiman-tmp-8-1"),
    ];
    for link in &aside {
        std::os::unix::fs::symlink(&old, link).unwrap();
    }
    let theirs = tool_dir.join(".notes.skiman-tmp-9");
    std::os::unix::fs::symlink(skills.join("docs/other"), &theirs).unwrap();

    std::fs::create_dir_all(skills.join("writing/long")).unwrap();
    let new = skills.join("writing/long/notes");
    std::fs::rename(&old, &new).unwrap();
    sandbox.skiman().arg("sync").assert().code(0);

    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("notes"), &new, &new);
    }
    for link in &aside {
        assert!(!link.is_symlink(), "{} is still there", link.display());
    }
    assert_eq!(
        std::fs::read_link(&theirs).unwrap(),
        skills.join("docs/other")
    );
}

/// A link named after a folder inside a skill, pointing there, is the
/// user's at any depth: `skills/<x>/<name>` is a skill in a group only when
/// no folder of `<x>` is a skill folder.
#[test]
fn a_link_into_a_folder_inside_a_skill_is_not_skimans() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skills = sandbox.canon().join("skills");
    write_skill(&skills, "pdf");
    write_skill(&skills.join("docs"), "fill");
    let tool_dir = sandbox.tool_dir(Tool::ClaudeCode);
    std::fs::create_dir_all(&tool_dir).unwrap();
    let theirs: Vec<_> = ["pdf/scripts", "pdf/scripts/run", "docs/fill/lib/forms"]
        .into_iter()
        .map(|inside| {
            let target = skills.join(inside);
            std::fs::create_dir_all(&target).unwrap();
            let link = tool_dir.join(target.file_name().unwrap());
            std::os::unix::fs::symlink(&target, &link).unwrap();
            (link, target)
        })
        .collect();

    sandbox.skiman().arg("sync").assert().code(0);

    for (link, target) in theirs {
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
    }
}

#[test]
fn while_another_process_holds_the_lock_sync_hook_exits_0_silently_and_new_fails_after_10_seconds()
{
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.canon().join("skills"), "notes");
    let lock = std::fs::File::options()
        .write(true)
        .open(sandbox.canon().join(".skiman/lock"))
        .unwrap();
    lock.lock().unwrap();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    assert!(
        std::fs::symlink_metadata(sandbox.tool_dir(Tool::ClaudeCode).join("notes")).is_err(),
        "a busy lock means sync --hook does nothing"
    );

    let started = Instant::now();
    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "d"])
        .assert()
        .code(4)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: another skiman command holds [..]/canon/.skiman/lock (waited 10 seconds)

"#]]);
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(9_500) && waited < Duration::from_secs(20),
        "waited {waited:?}"
    );
    assert!(!sandbox.canon().join("skills/pdf").exists());

    drop(lock);
    sandbox
        .skiman()
        .args(["new", "pdf", "-d", "d"])
        .assert()
        .code(0);
}

#[test]
fn init_on_a_clone_on_a_second_machine_distributes_every_committed_skill() {
    let first = Sandbox::new();
    init(&first);
    for name in ["notes", "pdf"] {
        first
            .skiman()
            .args(["new", name, "-d", "d"])
            .assert()
            .code(0);
    }
    first.git(first.canon(), ["add", "--all"]);
    first.git(
        first.canon(),
        [
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.test",
            "commit",
            "--quiet",
            "-m",
            "skills",
        ],
    );

    let second = Sandbox::new();
    let clone = second.root().join("elsewhere/my-skills");
    second.git(
        second.root(),
        ["clone", "--quiet"]
            .map(std::ffi::OsString::from)
            .into_iter()
            .chain([first.canon().into(), clone.clone().into()]),
    );

    second
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&clone)
        .assert()
        .code(0);

    for name in ["notes", "pdf"] {
        let target = clone.join("skills").join(name);
        for tool in Tool::ALL {
            assert_symlink(second.tool_dir(tool).join(name), &target, &target);
        }
    }
    let manual = clone.join(".skiman/skiman-manual");
    assert_symlink(
        second.tool_dir(Tool::Codex).join("skiman-manual"),
        &manual,
        &manual,
    );
}

/// skiman notes a move to a backup in its state folder, never in an entry:
/// a skill holding a file named like the old note installs like any other,
/// and a note for a foreign skill that names no copy in the backups is
/// ignored.
#[test]
fn a_file_named_like_skimans_move_note_is_only_content() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let source = sandbox.root().join("src/bar");
    write_skill(source.parent().unwrap(), "bar");
    std::fs::write(source.join(".skiman-moved-to"), "/nonexistent/copy").unwrap();
    sandbox
        .skiman()
        .arg("add")
        .arg(&source)
        .arg("--yes")
        .assert()
        .code(0);
    assert!(
        sandbox
            .canon()
            .join("skills/bar/.skiman-moved-to")
            .is_file()
    );
    assert!(sandbox.canon().join("upstream/bar/SKILL.md").is_file());

    let tool_dir = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&tool_dir, "baz");
    let entry = tool_dir.join("baz");
    let meta = std::fs::symlink_metadata(&entry).unwrap();
    let note = moved_note(&sandbox.state_home(), &entry);
    write(note, format!("{} {}\n/elsewhere", meta.dev(), meta.ino()));
    let sync = sandbox.skiman().arg("sync").output().unwrap();
    let stderr = String::from_utf8_lossy(&sync.stderr);
    assert!(stderr.contains("foreign skill"), "{stderr}");
    let doctor = sandbox.skiman().arg("doctor").output().unwrap();
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(!stdout.contains("cut short"), "{stdout}");
}

/// A FIFO in what a move to another filesystem left is never read: the
/// session-start sync does not block on it (it would hold skiman's lock),
/// doctor does not say to delete what is left, and nothing is deleted.
#[test]
fn a_fifo_in_what_a_move_cut_short_left_does_not_block_sync_hook() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let tool_dir = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&tool_dir, "notes");
    let copy = sandbox
        .state_home()
        .join("skiman/backups/20200101T000000Z/claude-code/notes");
    write_skill(copy.parent().unwrap(), "notes");
    let entry = tool_dir.join("notes");
    let meta = std::fs::symlink_metadata(&entry).unwrap();
    let note = moved_note(&sandbox.state_home(), &entry);
    write(
        &note,
        format!("{} {}\n{}", meta.dev(), meta.ino(), copy.display()),
    );
    let made = sandbox
        .std_command("mkfifo")
        .arg(entry.join("pipe"))
        .status()
        .unwrap();
    assert!(made.success());

    let mut hook = sandbox
        .std_command(env!("CARGO_BIN_EXE_skiman"))
        .args(["sync", "--hook"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = hook.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(10) {
            hook.kill().unwrap();
            hook.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(
        status.and_then(|s| s.code()),
        Some(0),
        "sync --hook blocked"
    );
    assert!(exists(entry.join("pipe")) && entry.join("SKILL.md").is_file());
    assert!(note.is_file());
    // The copy holds no FIFO, so doctor does not say to delete the rest.
    let doctor = sandbox.skiman().arg("doctor").output().unwrap();
    let said = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        said.contains("changed since") && !said.contains("; delete it"),
        "{said}"
    );
}

/// One write at a time on a machine: a command that waited for the
/// machine lock while another switched the canon writes nothing.
#[test]
fn a_command_that_waited_while_the_canon_was_switched_writes_nothing() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let other = sandbox.root().join("other-repo");
    std::fs::create_dir(&other).unwrap();
    let config = sandbox.config_home().join("skiman/config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    let repo = sandbox.canon().display().to_string();
    let switched = text.replace(&repo, &other.display().to_string());
    assert_ne!(switched, text);
    let lock = std::fs::File::options()
        .write(true)
        .open(sandbox.state_home().join("skiman/lock"))
        .unwrap();
    lock.lock().unwrap();

    let output = std::thread::scope(|scope| {
        let new = scope.spawn(|| sandbox.skiman().args(["new", "pdf", "-d", "d"]).output());
        std::thread::sleep(Duration::from_millis(500));
        std::fs::write(&config, &switched).unwrap();
        drop(lock);
        new.join().unwrap().unwrap()
    });

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "the canon changed to {}",
            other.canonicalize().unwrap().display()
        )),
        "{stderr}"
    );
    assert!(!sandbox.canon().join("skills/pdf").exists());
}

/// `sync --hook` spawns no `git` (spec "Performance"), also when it has work:
/// it relinks a skill in every tool directory and mirrors a project item of
/// a root that is a git repo. A `git` first on the `PATH` logs each call and
/// runs the real one; an empty `PATH` would not do, since the hook exits 0
/// whatever fails.
#[test]
fn sync_hook_spawns_no_git_while_it_relinks_and_mirrors() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.canon().join("skills"), "notes");
    let work = sandbox.home().join("work");
    write(work.join("proj/AGENTS.md"), "v1\n");
    sandbox
        .command("git")
        .args(["init", "-q"])
        .current_dir(&work)
        .assert()
        .success();
    sandbox
        .skiman()
        .args(["project", "add", "--id", "work"])
        .arg(&work)
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["project", "adopt", "--yes"])
        .assert()
        .code(0);
    for tool in Tool::ALL {
        std::fs::remove_file(sandbox.tool_dir(tool).join("notes")).unwrap();
    }
    write(work.join("proj/AGENTS.md"), "v2\n");
    let (path, log) = logging_git(&sandbox);

    sandbox
        .skiman()
        .env("PATH", path)
        .args(["sync", "--hook"])
        .assert()
        .code(0);

    for tool in Tool::ALL {
        let target = sandbox.canon().join("skills/notes");
        assert_symlink(sandbox.tool_dir(tool).join("notes"), &target, &target);
    }
    let mirrored = sandbox.canon().join("projects/work/proj/AGENTS.md");
    assert_eq!(std::fs::read_to_string(mirrored).unwrap(), "v2\n");
    assert!(
        !exists(&log),
        "sync --hook ran git: {}",
        std::fs::read_to_string(&log).unwrap()
    );
}
