//! Sync ownership (spec Ownership): sync creates, retargets and removes only
//! entries it owns, leaves everything else alone and reports it once.

use std::path::{Path, PathBuf};

use snapbox::str;

use crate::support::{Sandbox, Tool, assert_symlink, init, tree};

fn write_skill(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    std::fs::write(
        dir.join(name).join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\n---\n"),
    )
    .unwrap();
}

fn own_skill(sandbox: &Sandbox, name: &str) -> PathBuf {
    write_skill(&sandbox.canon().join("skills"), name);
    sandbox.canon().join("skills").join(name)
}

fn sync_hook_stdout(sandbox: &Sandbox) -> String {
    let output = sandbox.skiman().args(["sync", "--hook"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "sync --hook always exits 0");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn a_foreign_skill_stays_untouched_and_sync_hook_reports_it_once_until_it_reappears() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&claude, "theirs");
    std::fs::write(claude.join("theirs/notes.txt"), "keep me").unwrap();
    let before = tree(claude.join("theirs"));

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skiman: foreign skill [..]/home/.claude/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from claude-code`

"#]])
        .stderr_eq("");
    assert_eq!(tree(claude.join("theirs")), before);

    assert_eq!(sync_hook_stdout(&sandbox), "");
    assert_eq!(tree(claude.join("theirs")), before);

    let aside = sandbox.root().join("aside");
    std::fs::rename(claude.join("theirs"), &aside).unwrap();
    assert_eq!(sync_hook_stdout(&sandbox), "");
    std::fs::rename(&aside, claude.join("theirs")).unwrap();
    assert!(
        sync_hook_stdout(&sandbox).contains("foreign skill"),
        "a foreign skill that disappears and reappears is new again"
    );
}

#[test]
fn sync_without_hook_reports_every_foreign_skill_on_stderr_every_time_and_exits_0() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.tool_dir(Tool::Codex), "theirs");

    for _ in 0..2 {
        sandbox
            .skiman()
            .arg("sync")
            .assert()
            .code(0)
            .stdout_eq("")
            .stderr_eq(str![[r#"
warning: foreign skill [..]/home/.agents/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from codex`

"#]]);
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn a_foreign_skill_with_the_name_of_a_repo_skill_is_not_replaced_and_sync_exits_3() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&claude, "pdf");
    let before = tree(&claude);
    let target = own_skill(&sandbox, "pdf");

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(3)
        .stdout_eq(str![[r#"
linked [..]/home/.agents/skills/pdf
linked [..]/home/.gemini/config/skills/pdf

"#]])
        .stderr_eq(str![[r#"
warning: [..]/home/.claude/skills/pdf is occupied by an entry skiman does not own, so skill pdf is not linked there; move it away, then run `skiman sync`

"#]]);

    assert_eq!(tree(&claude), before);
    assert_symlink(sandbox.tool_dir(Tool::Codex).join("pdf"), &target, &target);
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn a_regular_file_and_a_folder_without_skill_md_at_a_wanted_name_are_occupied_paths() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let codex = sandbox.tool_dir(Tool::Codex);
    std::fs::write(codex.join("notes"), "a file").unwrap();
    std::fs::create_dir(codex.join("pdf")).unwrap();
    std::fs::write(codex.join("pdf/readme.txt"), "no SKILL.md").unwrap();
    let before = tree(&codex);
    own_skill(&sandbox, "notes");
    own_skill(&sandbox, "pdf");

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(3)
        .stderr_eq(str![[r#"
warning: [..]/home/.agents/skills/notes is occupied by an entry skiman does not own, so skill notes is not linked there; move it away, then run `skiman sync`
warning: [..]/home/.agents/skills/pdf is occupied by an entry skiman does not own, so skill pdf is not linked there; move it away, then run `skiman sync`

"#]]);
    assert_eq!(tree(&codex), before);
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn sync_hook_exits_0_on_occupied_paths_and_reports_each_one_once() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    std::fs::write(sandbox.tool_dir(Tool::Codex).join("notes"), "a file").unwrap();
    own_skill(&sandbox, "notes");

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skiman: [..]/home/.agents/skills/notes is occupied by an entry skiman does not own, so skill notes is not linked there; move it away, then run `skiman sync`

"#]]);
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn links_into_another_tool_directory_and_copies_with_identical_content_are_foreign() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    let codex = sandbox.tool_dir(Tool::Codex);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&claude, "theirs");
    std::os::unix::fs::symlink(claude.join("theirs"), codex.join("theirs")).unwrap();
    std::os::unix::fs::symlink(claude.join("pdf"), codex.join("via-claude")).unwrap();
    std::fs::create_dir(codex.join("pdf-copy")).unwrap();
    std::fs::copy(target.join("SKILL.md"), codex.join("pdf-copy/SKILL.md")).unwrap();
    let before = tree(&codex);

    let output = sandbox.skiman().arg("sync").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8(output.stderr).unwrap();
    for name in ["pdf-copy", "theirs", "via-claude"] {
        assert!(
            stderr.contains(&format!(
                "foreign skill {} is not managed",
                codex.join(name).display()
            )),
            "{name} is foreign: {stderr}"
        );
    }
    assert_eq!(tree(&codex), before);
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn managed_elsewhere_entries_stay_untouched_and_are_never_reported() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let home = sandbox.home();
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    let codex = sandbox.tool_dir(Tool::Codex);
    // Each one would be a foreign skill without the managed-elsewhere rule.
    write_skill(&claude, "synced");
    write_skill(&claude, ".trash");
    write_skill(&home.join(".codex/skills"), ".system");
    write_skill(
        &home.join(".claude/plugins/cache/acme/skills"),
        "claude-plugin-skill",
    );
    write_skill(
        &home.join(".codex/plugins/acme/skills"),
        "codex-plugin-skill",
    );
    std::os::unix::fs::symlink(
        home.join(".claude/plugins/cache/acme/skills/claude-plugin-skill"),
        codex.join("claude-plugin-skill"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        home.join(".codex/plugins/acme/skills/codex-plugin-skill"),
        claude.join("codex-plugin-skill"),
    )
    .unwrap();
    write_skill(&codex, "bundle");
    std::fs::create_dir(codex.join("bundle/.claude-plugin")).unwrap();
    std::fs::write(codex.join("bundle/.claude-plugin/plugin.json"), "{}").unwrap();
    let before = tree(&home);

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    assert_eq!(sync_hook_stdout(&sandbox), "");

    assert_eq!(tree(&home), before);
}

#[test]
fn after_a_hand_delete_sync_removes_the_dangling_owned_links() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    std::fs::remove_dir_all(&target).unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
removed [..]/home/.claude/skills/pdf
removed [..]/home/.agents/skills/pdf
removed [..]/home/.gemini/config/skills/pdf

"#]])
        .stderr_eq("");
    for tool in Tool::ALL {
        assert!(std::fs::symlink_metadata(sandbox.tool_dir(tool).join("pdf")).is_err());
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn after_a_hand_rename_of_an_own_skill_sync_removes_the_old_link_and_creates_the_new_one() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let old = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    let new = sandbox.canon().join("skills/pdf-tools");
    std::fs::rename(&old, &new).unwrap();
    std::fs::write(
        new.join("SKILL.md"),
        "---\nname: pdf-tools\ndescription: d\n---\n",
    )
    .unwrap();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("");

    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert!(std::fs::symlink_metadata(dir.join("pdf")).is_err());
        assert_symlink(dir.join("pdf-tools"), &new, &new);
    }
}

#[test]
fn enabled_false_removes_the_links_and_enabled_true_recreates_them() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    let manifest = sandbox.canon().join("skiman.toml");
    let tools = std::fs::read_to_string(&manifest).unwrap();

    std::fs::write(
        &manifest,
        format!("{tools}\n[skills.pdf]\nenabled = false\n"),
    )
    .unwrap();
    sandbox.skiman().arg("sync").assert().code(0);
    for tool in Tool::ALL {
        assert!(std::fs::symlink_metadata(sandbox.tool_dir(tool).join("pdf")).is_err());
    }

    std::fs::write(
        &manifest,
        format!("{tools}\n[skills.pdf]\nenabled = true\n"),
    )
    .unwrap();
    sandbox.skiman().arg("sync").assert().code(0);
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("pdf"), &target, &target);
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn a_skill_whose_skill_md_has_broken_frontmatter_stays_linked() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    std::fs::write(
        target.join("SKILL.md"),
        "---\nname: [unclosed\nno end marker\n",
    )
    .unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("pdf"), &target, &target);
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn folders_under_skills_that_are_not_distributable_are_not_linked_and_sync_hook_reports_them_once()
{
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skills = sandbox.canon().join("skills");
    write_skill(&skills, "Bad_Name");
    write_skill(&skills, "synced");
    std::fs::create_dir(skills.join("no-skill-md")).unwrap();
    std::fs::write(skills.join("no-skill-md/README.md"), "x").unwrap();
    // An empty folder is an empty group folder: nothing to report.
    std::fs::create_dir(skills.join("empty")).unwrap();
    std::fs::write(skills.join(".DS_Store"), "x").unwrap();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skiman: skill folder [..]/canon/skills/Bad_Name is not distributed: its name is not a valid skill name
skiman: skill folder [..]/canon/skills/no-skill-md is not distributed: it holds no SKILL.md
skiman: skill folder [..]/canon/skills/synced is not distributed: its name is reserved

"#]])
        .stderr_eq("");
    assert_eq!(sync_hook_stdout(&sandbox), "");

    for tool in Tool::ALL {
        let names: Vec<_> = tree(sandbox.tool_dir(tool)).into_keys().collect();
        assert_eq!(names, [PathBuf::from("skiman-manual")]);
    }
}

#[test]
fn two_tool_directories_that_resolve_to_one_real_directory_are_reconciled_once() {
    let sandbox = Sandbox::new();
    let gemini = sandbox.tool_dir(Tool::Antigravity);
    std::fs::remove_dir(&gemini).unwrap();
    std::os::unix::fs::symlink(sandbox.tool_dir(Tool::Codex), &gemini).unwrap();
    init(&sandbox);
    write_skill(&sandbox.tool_dir(Tool::Codex), "theirs");
    let target = own_skill(&sandbox, "pdf");

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
linked [..]/home/.claude/skills/pdf
linked [..]/home/.agents/skills/pdf

"#]])
        .stderr_eq(str![[r#"
warning: foreign skill [..]/home/.agents/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from codex`

"#]]);
    assert_symlink(gemini.join("pdf"), &target, &target);
    let before = tree(sandbox.home());

    for _ in 0..2 {
        sandbox.skiman().arg("sync").assert().code(0).stdout_eq("");
        assert_eq!(sync_hook_stdout(&sandbox), "");
    }
    assert_eq!(tree(sandbox.home()), before);
}

#[test]
fn removing_a_tool_from_tools_enabled_removes_only_skimans_links_from_its_tool_directory() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    let gemini = sandbox.tool_dir(Tool::Antigravity);
    write_skill(&gemini, "theirs");
    std::fs::write(gemini.join("loose.txt"), "x").unwrap();
    std::os::unix::fs::symlink("/nowhere", gemini.join("dangling")).unwrap();
    std::fs::write(
        sandbox.canon().join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\", \"codex\"]\n",
    )
    .unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
removed [..]/home/.gemini/config/skills/pdf
removed [..]/home/.gemini/config/skills/skiman-manual
removed [..]/home/.gemini/GEMINI.md

"#]])
        .stderr_eq("");

    let names: Vec<_> = std::fs::read_dir(&gemini)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(names, ["dangling", "loose.txt", "theirs"]);
    assert!(sandbox.tool_dir(Tool::Codex).join("pdf").exists());
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn init_pointed_at_a_different_canon_retargets_or_removes_the_links_of_the_previous_one() {
    let sandbox = Sandbox::new();
    let first = sandbox.root().join("first-repo");
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&first)
        .assert()
        .code(0);
    for name in ["notes", "pdf"] {
        write_skill(&first.join("skills"), name);
    }
    sandbox.skiman().arg("sync").assert().code(0);
    let second = sandbox.canon();
    std::fs::create_dir_all(second.join("skills")).unwrap();
    sandbox.git(&second, ["init", "--quiet"]);
    std::fs::write(
        second.join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n",
    )
    .unwrap();
    write_skill(&second.join("skills"), "pdf");

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&second)
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
added .skiman/ to [..]/canon/.gitignore
added /linked/ to [..]/canon/.gitignore
installed git hook [..]/canon/.git/hooks/pre-commit
installed git hook [..]/canon/.git/hooks/post-commit
installed git hook [..]/canon/.git/hooks/post-merge
installed git hook [..]/canon/.git/hooks/post-checkout
installed git hook [..]/canon/.git/hooks/post-rewrite
removed [..]/home/.claude/skills/notes
retargeted [..]/home/.claude/skills/pdf
retargeted [..]/home/.claude/skills/skiman-manual
removed [..]/home/.agents/skills/notes
retargeted [..]/home/.agents/skills/pdf
retargeted [..]/home/.agents/skills/skiman-manual
removed [..]/home/.gemini/config/skills/notes
retargeted [..]/home/.gemini/config/skills/pdf
retargeted [..]/home/.gemini/config/skills/skiman-manual
retargeted [..]/home/.claude/CLAUDE.md
retargeted [..]/home/.codex/AGENTS.md
retargeted [..]/home/.gemini/GEMINI.md

"#]])
        .stderr_eq(str![[r#"
warning: the shared body [..]/canon/instructions/shared.md is missing, so every instruction file linked to it leads nowhere; make it a file (it may be empty)

"#]]);

    let pdf = second.join("skills/pdf");
    let manual = second.join(".skiman/skiman-manual");
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert_symlink(dir.join("pdf"), &pdf, &pdf);
        assert_symlink(dir.join("skiman-manual"), &manual, &manual);
        assert!(std::fs::symlink_metadata(dir.join("notes")).is_err());
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn init_with_a_foreign_skill_points_to_import_when_the_canon_has_no_skills() {
    let sandbox = Sandbox::new();
    write_skill(&sandbox.tool_dir(Tool::ClaudeCode), "theirs");

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.canon())
        .assert()
        .code(0)
        .stderr_eq(str![[r#"
warning: foreign skill [..]/home/.claude/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from claude-code`
hint: the canon has no skills yet and the tool directories or instruction locations hold content skiman does not own; bring your existing setup in with `skiman import` (see `skiman import --guide`)

"#]]);
}

#[test]
fn init_with_a_foreign_skill_points_to_adopt_when_the_canon_has_skills() {
    let sandbox = Sandbox::new();
    write_skill(&sandbox.tool_dir(Tool::ClaudeCode), "theirs");
    let repo = sandbox.canon();
    std::fs::create_dir_all(repo.join("skills")).unwrap();
    sandbox.git(&repo, ["init", "--quiet"]);
    std::fs::write(
        repo.join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\"]\n",
    )
    .unwrap();
    write_skill(&repo.join("skills"), "pdf");

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .assert()
        .code(0)
        .stderr_eq(str![[r#"
warning: foreign skill [..]/home/.claude/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from claude-code`
warning: the shared body [..]/canon/instructions/shared.md is missing, so every instruction file linked to it leads nowhere; make it a file (it may be empty)
hint: the tool directories hold foreign skills; bring each one into the canon with `skiman adopt <name> --from <tool id>`

"#]]);
}

#[test]
fn a_relative_link_to_the_skill_is_owned_and_resolves_against_the_real_tool_directory() {
    let sandbox = Sandbox::new();
    let codex = sandbox.tool_dir(Tool::Codex);
    let gemini = sandbox.tool_dir(Tool::Antigravity);
    std::fs::remove_dir(&codex).unwrap();
    std::os::unix::fs::symlink(&gemini, &codex).unwrap();
    init(&sandbox);
    own_skill(&sandbox, "pdf");
    // The kernel resolves it against `home/.gemini/config/skills`, not against
    // the alias `home/.agents/skills` that sync reaches the directory through.
    std::os::unix::fs::symlink(
        "../../../../canon/skills/../skills/./pdf",
        codex.join("pdf"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        "../../.claude/skills/../../../canon/skills/pdf",
        sandbox.tool_dir(Tool::ClaudeCode).join("pdf"),
    )
    .unwrap();
    assert!(codex.join("pdf/SKILL.md").is_file());
    let before = tree(sandbox.home());

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    assert_eq!(tree(sandbox.home()), before);
}

/// The sync warning and doctor line for `path`, which resolves to `target`
/// in the canon (`into`: "inside the canon" or "which is the canon").
fn guarded_line(path: &Path, target: &Path, into: &str) -> String {
    format!(
        "{} resolves to {}, {into}, so skiman writes nothing there; point it elsewhere, then run the command again",
        path.display(),
        target.display()
    )
}

/// Runs `skiman sync` (exit 3, the one warning `line`) and doctor (exit 3,
/// the same line).
fn assert_guarded(sandbox: &Sandbox, line: &str) {
    let sync = sandbox.skiman().arg("sync").assert().code(3);
    let stderr = String::from_utf8(sync.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.lines().any(|l| l == format!("warning: {line}")),
        "{stderr}"
    );
    let doctor = sandbox.skiman().arg("doctor").assert().code(3);
    let stdout = String::from_utf8(doctor.get_output().stdout.clone()).unwrap();
    assert!(stdout.lines().any(|l| l == line), "{stdout}");
}

/// A tool directory that resolves into the canon gets no link
/// and no "move it away" for the canon's own skills; nor does one in the
/// canon's git folder.
#[test]
fn sync_writes_nothing_into_a_tool_directory_that_resolves_into_the_canon() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.canon().join("skills"), "notes");
    sandbox.skiman().arg("sync").assert().code(0);
    let dir = sandbox.tool_dir(Tool::ClaudeCode);
    std::fs::remove_dir_all(&dir).unwrap();
    for into in ["skills", ".git/hooks"] {
        let target = sandbox.canon().join(into);
        let _ = std::fs::remove_file(&dir);
        std::os::unix::fs::symlink(&target, &dir).unwrap();
        let before = tree(&target);

        assert_guarded(&sandbox, &guarded_line(&dir, &target, "inside the canon"));

        assert_eq!(tree(&target), before, "{into}");
    }
}

/// `~/.claude` a link to the canon: no `CLAUDE.md` lands beside
/// `skiman.toml`. `~/.claude` a link to a folder in the canon that the canon
/// does not use (dotfiles kept in the canon) works as usual.
#[test]
fn sync_writes_no_instruction_link_beside_the_canons_records() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let claude = sandbox.presence_dir(Tool::ClaudeCode);
    std::fs::remove_dir_all(&claude).unwrap();
    let canon = sandbox.canon();
    std::os::unix::fs::symlink(&canon, &claude).unwrap();

    let file = sandbox.instruction_file(Tool::ClaudeCode);
    let line = guarded_line(&file, &canon.join("CLAUDE.md"), "inside the canon");
    assert_guarded(&sandbox, &line);
    assert!(std::fs::symlink_metadata(canon.join("CLAUDE.md")).is_err());

    std::fs::remove_file(&claude).unwrap();
    std::fs::create_dir_all(canon.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink(canon.join(".claude"), &claude).unwrap();
    sandbox.skiman().arg("sync").assert().code(0).stderr_eq("");
    let shared = canon.join("instructions/shared.md");
    assert_symlink(canon.join(".claude/CLAUDE.md"), &shared, &shared);
}

/// The canon made in a tool's own folder (`skiman init ~/.agents`): sync
/// writes nothing into it and says to keep the canon in a folder of its own,
/// not to point a link elsewhere, since there is none. A tool directory
/// that is a link into that folder (`~/.gemini/config/skills ->
/// ~/.agents/skills`) is not served through it while `skills/` is missing:
/// sync never makes a folder it refuses, so doctor and sync both say the
/// link leads nowhere.
#[test]
fn a_canon_in_a_tool_directory_is_refused_in_its_own_words() {
    let sandbox = Sandbox::new();
    let agents = sandbox.home().join(".agents");
    let gemini = sandbox.tool_dir(Tool::Antigravity);
    std::fs::remove_dir_all(&agents).unwrap();
    std::fs::remove_dir_all(&gemini).unwrap();
    std::os::unix::fs::symlink(agents.join("skills"), &gemini).unwrap();
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&agents)
        .assert()
        .code(3);
    // A canon cloned before it held a skill has no `skills/`.
    std::fs::remove_dir(agents.join("skills")).unwrap();
    let dir = sandbox.tool_dir(Tool::Codex);
    let line = format!(
        "{} is inside the canon, so skiman writes nothing there; keep the canon in a folder of its own",
        dir.display()
    );

    let out = crate::doctor::assert_doctor_foresees_sync(&sandbox);

    assert!(out.lines().any(|l| l == line), "{out}");
    let nowhere = format!("cannot use {} as a tool directory", gemini.display());
    assert!(out.contains(&nowhere), "{out}");
}

/// A canon made right in a tool directory is named as the canon, not as a
/// path that resolves into it.
#[test]
fn a_canon_made_in_a_tool_directory_says_it_is_the_canon() {
    let sandbox = Sandbox::new();
    let dir = sandbox.tool_dir(Tool::ClaudeCode);
    std::fs::remove_dir_all(&dir).unwrap();
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&dir)
        .assert()
        .code(3);
    let line = format!(
        "{} is the canon, so skiman writes nothing there; keep the canon in a folder of its own",
        dir.display()
    );

    let out = crate::doctor::assert_doctor_foresees_sync(&sandbox);

    assert!(out.lines().any(|l| l == line), "{out}");
}

/// A tool directory made between sync's first look and its write is read,
/// not taken as empty: a skill someone put there is reported as foreign.
#[test]
fn a_tool_directory_made_while_sync_looks_is_read() {
    let sandbox = Sandbox::builder()
        .tool(Tool::Codex, crate::support::ToolSetup::PresenceOnly)
        .build();
    init(&sandbox);
    let dir = sandbox.tool_dir(Tool::Codex);
    let _ = std::fs::remove_dir_all(&dir);
    let theirs = dir.join("theirs");
    let run = format!(
        "mkdir -p '{}' && printf -- '---\\nname: theirs\\ndescription: d\\n---\\n' > '{}/SKILL.md'",
        theirs.display(),
        theirs.display()
    );

    let sync = sandbox
        .skiman()
        .arg("sync")
        .env("SKIMAN_TEST_HOOK", format!("sync:create:{}", dir.display()))
        .env("SKIMAN_TEST_HOOK_RUN", run)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&sync.stderr);
    assert!(
        stderr.contains(&format!("foreign skill {}", theirs.display())),
        "{sync:?}"
    );
}
